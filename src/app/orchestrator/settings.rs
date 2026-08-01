//! Application configuration and (re)launching the inference/embedding servers through
//! [`ServerSupervisor`]. The orchestrator is the sole writer of `settings.json`.

use crate::app::events::AppEvent;
use crate::shared::config::{AppConfig, CloudProvider, ImpersonationMode, ServerMode};
use crate::shared::server::ServerStatus;

use super::Orchestrator;
use super::engines::Server;

impl Orchestrator {
    /// Applies configuration edits: saves, restarts the server/registry if
    /// needed, and re-emits the settings. The sole writer of `settings.json`.
    pub(super) fn handle_update_config(&mut self, config: AppConfig) {
        let old = std::mem::replace(&mut self.config, config);
        // The "last-open chat" is an orchestrator property, not a value editable on
        // the settings screen. The config snapshot from the UI may carry a stale
        // value (e.g. `None` from startup) — restore the actual one, so an edit to
        // settings doesn't erase the memory of the chat.
        self.config.last_active_chat = old.last_active_chat;
        // Stored API keys are also an orchestrator property (`handle_set_api_key`):
        // the settings screen sends the key itself as a separate command, and the config
        // snapshot never carries them. Without restoring it, editing any setting would wipe the keys.
        self.config.api_keys = old.api_keys.clone();
        // MCP catalog TOFU pins are also an orchestrator property (`persist_mcp_pin`),
        // not editable in the UI: inherit by server id when the UI snapshot doesn't
        // carry them (a stale copy) — editing settings doesn't reset trust or
        // trigger a false `config.mcp` diff (an extra server restart).
        for srv in &mut self.config.mcp.servers {
            if srv.pinned_catalog.is_none()
                && let Some(prev) = old.mcp.servers.iter().find(|s| s.id == srv.id)
            {
                srv.pinned_catalog = prev.pinned_catalog.clone();
            }
        }
        if let Err(err) = self.storage.json().save_config(&self.config) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.save_settings_failed", &[("err", &err.to_string())]),
            ));
            self.config = old; // roll back to the previous state
            return;
        }
        // A chat-server settings change (model/mode/port/…) — a restart (spec
        // §11.6), but deferred: the settings screen applies an edit on every field's
        // commit, and the debounce coalesces a series of quick edits into one restart
        // ([`super::restart_queue::RestartQueue`], flushed at the deadline in the loop).
        if self.config.engine != old.engine {
            self.restarts.mark_chat();
            // A mode change alters the set of available sampling parameters
            // (get_sampling/set_sampling) — rebuild the registry for the new
            // provider right away (cheap, in-memory; the schema needs to be
            // current starting from the very next turn).
            if self.config.engine.mode.cloud_provider() != old.engine.mode.cloud_provider() {
                self.rebuild_registry();
            }
        }
        // An impersonation-server settings change — a deferred reconnect.
        if self.config.impersonation_engine != old.impersonation_engine {
            self.restarts.mark_impersonation();
        }
        // An embedding-server settings change — a deferred reconnect.
        if self.config.embed != old.embed {
            self.restarts.mark_embed();
        }
        // An MCP-servers settings change — a deferred re-raise (a debounce, like
        // the engines): killing/spawning processes is an expensive operation.
        if self.config.mcp != old.mcp {
            self.restarts.mark_mcp();
        }
        // A tool-parameter change — a registry rebuild (python_path, limits).
        // Video settings feed the same registry (the `youtube_watch` client is
        // built there), so they rebuild it too.
        if self.config.tools != old.tools || self.config.video != old.video {
            self.rebuild_registry();
        }
        self.emit_settings();
    }

    /// Saves an API key entered in settings for the provider: encrypts it with the
    /// machine key (`shared::secrets`) and puts it into `config.api_keys` as **this**
    /// machine's entry. An empty key means removal. Affects every slot (chat/impersonation/
    /// embeddings) whose active provider is this one: they're flagged for a deferred
    /// (re)raise — the key is picked up by the next `flush_restarts`.
    ///
    /// The plaintext lives only in the argument and in the HTTP client: what goes to
    /// disk is the ciphertext, and the UI's config snapshot doesn't carry the keys at
    /// all (the UI only gets a "configured" flag). See docs/research/api-key-storage.md.
    pub(super) fn handle_set_api_key(&mut self, provider: CloudProvider, key: String) {
        if !self.store_secret(provider.key(), &key) {
            return;
        }
        // Re-raise only the servers whose active provider had its key changed
        // (the same debounce as for engine edits — see `flush_restarts`).
        let p = Some(provider);
        if self.config.engine.mode.cloud_provider() == p {
            self.restarts.mark_chat();
        }
        if self.config.impersonation_engine.mode.cloud_provider() == p {
            self.restarts.mark_impersonation();
        }
        if self.config.embed.mode.cloud_provider() == p {
            self.restarts.mark_embed();
        }
        // The video slot uses the Gemini key too, and its client lives in the
        // tool registry — so a Gemini key change has to rebuild it (cheap,
        // in-memory), or `youtube_watch` would keep reporting itself
        // unconfigured until the next unrelated settings edit.
        if provider == CloudProvider::Gemini {
            self.rebuild_registry();
        }
        self.emit_settings();
    }

    /// Stores the backup password for **this** machine (spec §12.3). Same storage
    /// and the same never-shown-again contract as an API key; nothing has to be
    /// restarted, so it just persists and re-emits the presence flag.
    pub(super) fn handle_set_backup_password(&mut self, password: String) {
        if self.store_secret(crate::shared::secrets::BACKUP_PASSWORD_KEY, &password) {
            self.emit_settings();
        }
    }

    /// Encrypts a secret into this machine's entry and persists the config.
    /// `false` — it failed and the error was already reported to the user; the
    /// previous state is restored, so a failed save never half-applies.
    fn store_secret(&mut self, name: &str, value: &str) -> bool {
        let old = self.config.api_keys.clone();
        let label = || {
            format!(
                "{} · {}",
                crate::shared::secrets::machine_label(),
                chrono::Local::now().format("%Y-%m-%d")
            )
        };
        if let Err(err) =
            crate::shared::secrets::put_key(&mut self.config.api_keys, name, value.trim(), label)
        {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.api_key_save_failed", &[("err", &err.to_string())]),
            ));
            return false;
        }
        if let Err(err) = self.storage.json().save_config(&self.config) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.save_settings_failed", &[("err", &err.to_string())]),
            ));
            self.config.api_keys = old; // roll back to the previous state
            return false;
        }
        true
    }

    /// Applies (re)launches of servers that were deferred by the debounce (the deadline expired):
    /// one `apply_*` per flagged server, and one shared status
    /// snapshot. Reads the **final** `self.config` — the config is already replaced during
    /// the edit, so a series of edits produces one restart with the final values.
    pub(super) fn flush_restarts(&mut self) {
        let (chat, embed, imp, mcp) = self.restarts.take();
        let loc = self.ui_locale();
        let keys = &self.config.api_keys;
        // The flag only says "something was edited"; whether a restart is *worth doing*
        // is decided here, against what each server is actually running. An edit and its
        // undo (`Ctrl+Z`) both raise the flag, and the final config then equals the
        // applied one — restarting would reload the server with the values it already
        // has, and for a managed one that means killing and reloading a GGUF for
        // nothing. See docs/history/settings-undo.md §5.1.
        let mut applied_any = false;
        if chat && !self.engines.chat_is_current(&self.config.engine, keys) {
            self.engines.apply_chat(&self.config.engine, keys, loc);
            applied_any = true;
        }
        if embed && !self.engines.embed_is_current(&self.config.embed, keys) {
            self.engines.apply_embed(&self.config.embed, keys, loc);
            applied_any = true;
        }
        if imp
            && !self
                .engines
                .impersonation_is_current(&self.config.impersonation_engine, keys)
        {
            self.engines
                .apply_impersonation(&self.config.impersonation_engine, keys, loc);
            applied_any = true;
        }
        if mcp && !self.mcp.is_current(&self.config.mcp) {
            self.apply_mcp_settings();
        }
        if applied_any {
            self.emit_server_status();
        }
    }

    /// (Re-)raises the chat server from `config.engine` and emits a status snapshot.
    /// The immediate startup path (before the `run` loop); settings edits
    /// go through the `restarts` debounce queue → [`Self::flush_restarts`].
    pub(super) fn apply_chat_settings(&mut self) {
        let loc = self.ui_locale();
        self.engines
            .apply_chat(&self.config.engine, &self.config.api_keys, loc);
        self.emit_server_status();
    }

    /// (Re-)raises the embedding server from `config.embed` and emits a status snapshot
    /// (the embeddings chip in the status line appears/disappears based on the setting).
    pub(super) fn apply_embed_settings(&mut self) {
        let loc = self.ui_locale();
        self.engines
            .apply_embed(&self.config.embed, &self.config.api_keys, loc);
        // Two decorators, and the order is load-bearing:
        //
        //   EmbedGuard { PrefixedEmbedder { real embedder } }
        //
        // The prefixer applies the model's input convention (`query:`/`passage:`
        // and relatives). It goes *inside* the guard so the guard's own canary
        // and calibration probes pass through it: that is what makes a
        // convention switch read as the change of vector space it really is, and
        // what keeps the similarity calibration measured in the same dressing
        // the real text gets (docs/research/embedding-input-prefixes.md §3–§4).
        //
        // The guard itself: stored vectors are only comparable to a query from
        // the same model, and dimensionality cannot establish that (see
        // `embed_guard`). Rebuilding both here re-arms the check whenever the
        // embedding settings change — exactly when the model is most likely to
        // have been swapped. The check is lazy (embeddings have no readiness
        // probe, ADR 0002).
        let prefixed = std::sync::Arc::new(crate::shared::embed_prefix::PrefixedEmbedder::new(
            self.engines.embedder.clone(),
            self.config.embed.convention,
        ));
        self.engines.embedder = std::sync::Arc::new(super::embed_guard::EmbedGuard::new(
            prefixed,
            self.storage.clone(),
            self.config.embed.active_model_name(),
            self.config.embed.convention,
            self.ui_locale(),
            self.evt_tx.clone(),
        ));
        self.emit_server_status();
    }

    /// Revives a **managed** server whose process is gone.
    ///
    /// Only managed servers are relaunched: we own the process, and a dead child
    /// leaves a port that no amount of probing will revive. An external or cloud
    /// server is someone else's to restart — its monitor keeps polling and picks the
    /// recovery up on its own.
    ///
    /// Called after every status update, so a relaunch that fails simply produces the
    /// next `Disconnected` and the next attempt, until [`RestartBudget`] stops it. A
    /// launch that fails *synchronously* (a missing model file — the preflight check)
    /// posts no status at all and therefore never reaches this path: retrying it would
    /// be pointless until the settings change. See docs/server-health-monitoring.md, F4.
    ///
    /// [`RestartBudget`]: super::engines::RestartBudget
    pub(super) fn relaunch_dead_managed_servers(&mut self) {
        let loc = self.ui_locale();
        let now = std::time::Instant::now();
        let mut relaunched = false;

        let chat_managed = self.config.engine.mode == ServerMode::Managed;
        if chat_managed && self.needs_relaunch(Server::Chat, now) {
            tracing::warn!("managed chat server is down — relaunching");
            self.engines
                .apply_chat(&self.config.engine, &self.config.api_keys, loc);
            relaunched = true;
        }
        if self.config.embed.mode == ServerMode::Managed && self.needs_relaunch(Server::Embed, now)
        {
            tracing::warn!("managed embedding server is down — relaunching");
            self.engines
                .apply_embed(&self.config.embed, &self.config.api_keys, loc);
            relaunched = true;
        }
        if self.config.impersonation_engine.mode == ImpersonationMode::Managed
            && self.needs_relaunch(Server::Impersonation, now)
        {
            tracing::warn!("managed impersonation server is down — relaunching");
            self.engines.apply_impersonation(
                &self.config.impersonation_engine,
                &self.config.api_keys,
                loc,
            );
            relaunched = true;
        }
        if relaunched {
            self.emit_server_status(); // the chip returns to "connecting…"
        }
    }

    /// Whether `server` is down and its crash-loop budget still allows a relaunch.
    /// Consumes a budget slot when it answers `true`.
    fn needs_relaunch(&mut self, server: Server, now: std::time::Instant) -> bool {
        if !matches!(
            self.engines.status_of(server),
            ServerStatus::Disconnected(_)
        ) {
            return false;
        }
        if self.engines.allow_relaunch(server, now) {
            return true;
        }
        tracing::warn!(
            ?server,
            "relaunch budget exhausted — leaving it disconnected"
        );
        false
    }

    /// (Re-)raises the impersonation server from `config.impersonation_engine`. In
    /// `shared` mode a separate server isn't needed — the assistant's chat server is reused.
    pub(super) fn apply_impersonation_settings(&mut self) {
        let loc = self.ui_locale();
        self.engines.apply_impersonation(
            &self.config.impersonation_engine,
            &self.config.api_keys,
            loc,
        );
        self.emit_server_status();
    }

    /// (Re-)raises MCP servers from `config.mcp`: previous ones are killed, enabled
    /// ones are spawned anew; their tools arrive via `Ready` events (see
    /// [`super::mcp::McpManager`]). The registry is rebuilt right away — wrappers from the previous
    /// generation (dead connections) leave it immediately.
    pub(super) fn apply_mcp_settings(&mut self) {
        let loc = self.ui_locale();
        self.mcp.apply(&self.config.mcp, loc);
        self.rebuild_registry();
    }
}
