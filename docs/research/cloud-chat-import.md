# Importing chats exported from ChatGPT, Claude, Gemini, Grok and DeepSeek

Status: **research; forks decided by the owner on 2026-10-25** (§7). The
decisions:
- **As recommended:** F1 (a), F2 (a), F3 (a) with (b) as a flag, F4 (a),
  F5 (a), F8 (b).
- **Against the recommendation:**
  - **F6 (b) for images:** inline in the chat.
  - **F7 (b):** an imported chat is reflected on like any chat.
  - **F9 (a):** import everything, with no filters.
- **Clarified for F6:** files that are not images become the chat's files
  (F6 a).

Nothing is built. Next is the probe (§8, stage 0) on the owner's own exports.
The owner has accounts with all five services.

**No real export was opened for this document.** Every
format below comes from three kinds of evidence: the current code of parsers
written against real exports, redacted samples attached to their issues, and
the services' own help pages. Each claim is marked where it rests on one source,
or on none of the three. The probe (§8, stage 0) turns this into measurement.
Nothing here should be built before that.

## 1. Why

- **What the audience has.** Advanced users and AI researchers have years of
  conversations in the cloud apps. mindfork continues a
  conversation with any model: a local Gemma on a rented pod, or the same cloud
  model through its API. Today it starts every one of those conversations empty.
- **There is no API for this history.** An API key reaches the platform, not
  the history of chatgpt.com or claude.ai. Each service's **data export** is the
  one road to that history that breaks no terms. Most of these exports exist
  because of data-portability law (GDPR Art. 20).
- **Scraping is out of scope.** Browser extensions that read the live app's
  private endpoints are a different source: undocumented, tied to a session, and
  outside what this document considers.
- **What mindfork imports today.** `mindfork import <file>` reads the neutral
  `mindfork-import` v1 document and character cards, telling them apart by
  content (`main.rs::import_unguarded`).
- **What ADR 0007 decided.** [ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md)
  §3 sends every other source to an external converter. None exists for any of
  the five services.
- **The counter-example on record.** [plugin-system.md](plugin-system.md) §5.1
  names ChatGPT's `conversations.json` as what an exchange format must not be:
  "an undocumented tree of nodes that every consumer has to re-learn how to
  flatten". This document is about re-learning it, and four formats like it.

## 2. What each service gives — summary

How the export is obtained:

| Service | Where | Delivery | Container | Who can |
|---|---|---|---|---|
| **ChatGPT** | Settings → Data controls → Export data | Link by email/SMS, valid 24 h; ready within up to 7 days | ZIP. Since 2026 often nested ZIPs, sometimes a multi-GB outer ZIP that is not ZIP64 | Free, Plus, Pro, eligible Edu. **Not** Business/Enterprise |
| **Claude** | Settings → Privacy → Export data (web and desktop, not mobile) | Link by email, valid 24 h, sign-in required. Since ~Aug 2026 a **manifest JSON** linking to several ZIPs | ZIP(s) | Free/Pro/Max. Team/Enterprise members only if an owner allows |
| **Gemini app** | Google Takeout → My Activity → "Gemini Apps" (JSON must be chosen; HTML is the default) | Link valid 7 days, or a Drive/Dropbox/OneDrive/Box upload; hours to days | ZIP/TGZ, split past a chosen size | Personal accounts; Workspace if Takeout is allowed and history is on |
| **AI Studio** | The Drive folder "Google AI Studio" (download, rclone, or Takeout "Drive") | — | One JSON file per saved chat | Any account that saved chats |
| **Grok** | Settings → Data Controls → accounts.x.ai "Download account data" | Conflicting reports: an email link or a direct download | ZIP | Any grok.com account |
| **DeepSeek** | Settings → Data → Export data (web only) | A browser download, `deepseek_data-YYYY-MM-DD.zip` | ZIP | Any web account |

What the export carries:

| Service | Structure | Branch the user last saw | Thoughts | Tool activity | File bytes | Model per message |
|---|---|---|---|---|---|---|
| **ChatGPT** | A tree, `mapping` by `parent` | `current_node` | Summaries in their own nodes | Classic: tool nodes. Mid-2026 exports reportedly drop them | Usually, as `.dat` blobs plus a name map. Often missing since spring 2026 | `metadata.model_slug`, else the chat's `default_model_slug` |
| **Claude** | A list with `parent_message_uuid` | **None** — the newest leaf is a heuristic | `thinking` blocks, one sample truncated | `tool_use` / `tool_result` blocks | **Never** in a personal export. Text uploads' extracted text only | **None** |
| **Gemini app** | **An activity log**: one record per prompt, regrouped by the chat link | n/a (drafts as extra items) | None | None | Yes, files next to the log | None |
| **AI Studio** | Linear chunks; a branch is a separate file | n/a | `isThought` chunks | Code-execution parts | Inline base64 or Drive ids | `runSettings.model` (per chat) |
| **Grok** | A list with `parent_response_id` | `leaf_response_id`, rarely set; else the newest leaf | `thinking_trace`, `agent_thinking_traces` | Search results, `steps[]` | Uploads, yes. Generated images as URLs only | `model`, or `metadata` on the user turn |
| **DeepSeek** | A tree, `mapping` from `"root"` | **None** — the newest leaf | `THINK` fragments, in full | Search results (title, url), bare `TOOL_OPEN` | **None**, metadata only | `deepseek-chat` / `deepseek-reasoner` |

No service documents its format. ChatGPT changed its format at least five
times in 2026. Claude moved to a split delivery in August 2026. Grok has two
key/timestamp eras within 2025. DeepSeek moved files and tool steps into new
fragment types in the first half of 2026. **An importer for any of them is a
reader of a moving target.** That fact shapes the design (§6) more than any
field does.

## 3. Per service — what an importer has to know

### 3.1 ChatGPT

**The container has changed shape repeatedly:**
- 2023–2024: one ZIP with `conversations.json` (a JSON array), `chat.html` (an
  offline viewer, 160–255 MB in real exports), `user.json` and a few side files.
- 31 Mar 2025: media became top-level `.dat` blobs.
- Mid-Feb 2026: conversations split into **shards**, `conversations-000.json`,
  `-001`, …, about 100 conversations each, cut by id rather than by date. One
  account of 9,134 conversations came as 92 shards.
- Since about June 2026: an outer ZIP wraps `Conversations__…part-00N.zip` and
  `Files__…files-00N.zip`. One parser reports an outer ZIP **over 4 GiB that is
  not ZIP64**: its offsets wrap at 2³², and `unzip`, `bsdtar` and Python's
  `zipfile` all fail on it ([temnoon/openai_export_parser](https://github.com/temnoon/openai_export_parser),
  commit 5fbbbb6, 2 Jun 2026). Whether the `zip` crate (9.0.5, `Cargo.toml`)
  fails too is **unmeasured**.
- Since about August 2026: `export_manifest.json` lists the shards, and
  `conversation_asset_file_names.json` maps each blob to its original file name.

**Conversation:**
- Fields: `title`, `create_time`/`update_time` (**Unix seconds as floats**),
  `mapping`, `current_node`, `default_model_slug`, `conversation_template_id`,
  `is_archived`, `voice`, ….
- `mapping` holds nodes `{id, message|null, parent, children}`.
- **The branch the user last saw** is the walk from `current_node` up the
  `parent` links to the root. Every maintained parser reads it this way.
- **October 2026 exports have no `children`**
  ([marelucent/chatgpt-export-reader#3](https://github.com/marelucent/chatgpt-export-reader/issues/3),
  a real redacted sample). This broke convoviz
  ([#70](https://github.com/mohamed-chs/convoviz/issues/70)) and that reader.
  Build the tree from `parent` alone.
- **Pitfalls**: threads deeper than 1,200 nodes; cycles in crafted files
  (LibreChat added guards, PR #12313); bare `{}` entries; a tool result
  timestamped before its parent (so never sort by time).

**Message:**
- `author.role` is `system | user | assistant | tool`.
- `content.content_type` is one of:
  - `text`;
  - `multimodal_text`: strings mixed with `image_asset_pointer`,
    `audio_transcription` and `audio_asset_pointer` parts;
  - `code`, `execution_output`;
  - `tether_quote`, `tether_browsing_display`;
  - `thoughts`: `{thoughts:[{summary, content}]}`;
  - `reasoning_recap`: "Thought for 29s";
  - `user_editable_context`: custom instructions;
  - `model_editable_context`: the memory snapshot given to the model;
  - `computer_output`, and others.
- An assistant message whose `recipient` is not `"all"` is a tool call.
- GPT-5 turns carry `channel`: `final` for the answer, `commentary` for
  preambles.
- The model is in `metadata.model_slug`. By 2026-08 most messages omit it, so
  fall back to the conversation's `default_model_slug` (LibreChat PR #15388,
  open).
- **October 2026 messages also lack `recipient`**, so a tool call can no longer
  be told apart by its addressee.
- Answers carry private-use markers (U+E200–E202, e.g. `citeturn0search3`)
  that must be stripped or resolved.
- A chat branched into a new one carries `metadata.branching_from_conversation_id`.

**Thoughts:**
- These are **summaries**, never the raw chain of thought.
- They sit in their own nodes before the answer, so they fold into the next kept
  assistant reply.
- The one October 2026 sample held none. Whether trimmed exports still carry
  them is unknown.

**Tools:**
- The classic shapes:
  - code with `recipient: "python"`, followed by an `execution_output`;
  - search with `web.run`, its citations in `metadata.content_references`;
  - canvas as `canmore.create_textdoc` JSON;
  - memory writes to `bio`.
- **Exports from mid-2026 reportedly carry no `tool` and no `system` nodes.**
  Three independent parsers say so; nothing official does.

**Files:**
- Pointers are `file-service://file-…` (classic) and `sediment://file_…`
  (2025+).
- The bytes are `.dat` blobs named by id. The original names are in
  `conversation_asset_file_names.json`, and the type is told by magic bytes.
- References to files the archive does not hold are common.
- Since spring 2026 some exports miss generated images "and even entire
  conversation turns"; ChatKeeper 2026.10.002 warns about it (a vendor report).

**What the export does not hold:**
- The names and instructions of custom GPTs and projects. A project is only an
  id (`conversation_template_id` beginning `g-p-`).
- Account-level memory (unconfirmed either way).

**Scale seen:**

| Date | Size and contents |
|---|---|
| Feb 2026 | 2,808 conversations, 45,247 messages, a 155 MB `conversations.json`, a 775 MB ZIP |
| Sep 2025 | Over 500 MB, about 7,000 conversations |
| Jun 2026 | A 5 GB ZIP: 1,968 conversations, 5,936 media files |

### 3.2 Claude

**Delivery:**
- Since about 20 Aug 2026 the email links a **manifest**:
  `{version:"1.0", data_files:[{category, part, filename, export_url}]}`.
- The categories seen are `conversations`, `projects`, `memories`,
  `light_metadata`, `feedback` and `frames`. Each is its own ZIP.
- `conversations-000.zip` holds only `conversations.json`. `users.json` moved
  to another part.
- Before that date there was one ZIP: `conversations.json`, `users.json` and
  `projects.json`.
- Folders seen unpacked from these parts include `design_chats-000/`,
  `artifact_data-000/` and `claude_code_cloud-000/`.
- The person downloads every part. An importer would take the manifest, the
  ZIPs, or the folder they were unpacked into.

**Conversation:**
- `conversations.json` is a bare array of
  `{uuid, name, summary, created_at, updated_at, account, chat_messages[]}`.
- Times are ISO UTC with microseconds.
- `summary` is an English Markdown overview, written even for chats in other
  languages.
- **No model anywhere** in the bulk export, and no project link.

**Message:**
- Fields: `uuid`, `text`, `content[]`, `sender` (`human` | `assistant`),
  `created_at`, `attachments[]`, `files[]`, `parent_message_uuid`.
- **Read `content[]`, not `text`.** `text` is a flattened rendering that puts
  "This block is not supported on your current device yet." in a fence wherever
  a tool or thinking block was.
- Block types:
  - `text`, with citations;
  - `thinking` (`thinking`, `summaries[]`, `truncated`);
  - `tool_use` and `tool_result`. Web-search results are `knowledge` items with
    title, url and text;
  - `voice_note`.
- Since October 2025, unknown types appear (`injected_prompt_block`,
  `token_budget`). Skip them.

**Branches:**
- Every message has `parent_message_uuid`; there is **no current-leaf pointer**.
- **Abandoned edits and retries are present as siblings.** One parser, reading
  a real export, counts 136 conversations that hold both branches
  ([d3layd/agent-history PR #32](https://github.com/d3layd/agent-history/pull/32)).
  The redacted sample could not show it, because its ids were re-hashed.
- Cherry Studio takes the array's last message as the active leaf.
- Heuristics in use: the newest leaf by `created_at`, or the deepest path. One
  parser's test argues the newest leaf can pick an abandoned retry.

**Thinking:**
- It is present. In the one real sample it was cut short (`truncated: true`).
- It is most likely the summarized thinking Claude 4+ models return.

**Artifacts** came in three generations:
1. **2024:** inline `<antArtifact>` in `text`.
2. **2025:** the `artifacts` tool. `create` and `rewrite` carry the whole
   content, and `update` carries an `old_str` → `new_str` diff, so the final
   text is a replay.
3. **Late 2025–2026:** `create_file`, `str_replace` and `present_files`. Binary
   outputs exist only as `computer:///` links.

Anthropic's own "Import from Claude" wizard keeps the final artifact only.

**Files:**
- **Never in a personal export.** Text uploads (`txt`, `csv`, `docx`, `md`…)
  keep their full text in `attachments[].extracted_content`.
- Images are names in `files[]`.
- A Team/Enterprise member's export has no file contents at all. Only an
  organization export does (support.claude.com).

**Projects:**
- `projects.json` holds `name`, `description`, `prompt_template` (the
  instructions) and `docs[{filename, content}]`.
- Nothing links a chat to its project
  ([anthropics/claude-code#40411](https://github.com/anthropics/claude-code/issues/40411),
  closed as not planned).
- What the newer `projects` and `memories` parts hold is unverified.

**Scale seen:** 728 conversations in 44 MB; a 206 MB `conversations.json`.

### 3.3 Gemini app (Google Takeout)

**Where the data lands:**
- Chats are in `Takeout/My Activity/Gemini Apps/MyActivity.json`, with uploaded
  and generated files beside it.
- **Folder and file names are localized** (`MeineAktivitäten.json`, `我的活动`;
  "Bard" in old accounts), so the archive must be recognized by content.

**It is an activity log, not conversations.** Each record is one prompt:
- `title`: `Prompted <prompt>`, with the verb localized: `Prompted`,
  `Asked`, `プロンプト`, `Demande`…. No Russian verb is known.
- `time`: ISO UTC.
- `safeHtmlItem[{html}]`: **the reply as HTML**. The shown draft comes first,
  the other drafts after it.
- `subtitles[]` and `attachedFiles[]`.
- `details[]`, or `titleUrl`, carrying `https://gemini.google.com/app/<id>`.

**Rebuilding a conversation:**
- Group records by the chat id in that link, then sort by time.
- A record with no link becomes a chat of its own.
- **There are no titles, no model, no thoughts and no turn ids.**
- **Many records have no reply:** 29% in one export.
- A record kind "Branched" exists.
- No real record is public. This shape is triangulated from parsers' code:
  baip/chatbots-export, OTLFrostA's cleaned real fixture, and
  [Sinity/polylogue](https://github.com/Sinity/polylogue).

**What is lost:**
- Chats deleted in the app.
- Chats made while activity was off, and temporary chats (kept 72 h, never
  exported).
- Anything past the auto-delete window (18 months by default).

**Gems:** they come from the separate "Gemini" Takeout product. Their layout,
and whether instructions are included, are unverified.

**Prefer the JSON format.** HTML records carry localized date text with zone
abbreviations, and `CST` is ambiguous.

### 3.4 Google AI Studio

**Storage:**
- Each saved chat is one Drive file in "Google AI Studio".
- The file is named by its title, **with no extension**. Titles can end in a
  dot, which NTFS refuses.

**Format:**
- `runSettings`: `{model: "models/gemini-2.5-pro", temperature, topP, topK,
  maxOutputTokens, thinkingBudget, …}`.
- `systemInstruction`.
- `chunkedPrompt.chunks[]`, each with:
  - `role` (`user` | `model`), `text`, `isThought`, `parts[]`;
  - `driveImage{id}` or `inlineImage{mimeType, data}`;
  - `tokenCount`, `finishReason`, and `createTime` (absent in older files).
- `pendingInputs`: the unsent input box.
- Field names come from 228 real files and 10,885 chunks parsed by polylogue.

**Branches:** the chat itself is linear. A branch is a separate file linked by
`branchParent.promptId`.

**Quirks:**
- 1,685 of the 10,885 chunks carry the same content in `text` and in `parts`.
  Emit one, or the text doubles (polylogue PR #4764 measured +16.6%).
- Files with inline media are 9–203 MB.

**The best fit of the six.** It maps almost one to one onto `Chat`: the system
message, thoughts, images and the model. Its `runSettings` even map onto
`SamplingConfig`.

### 3.5 Grok

**The ZIP:**
- Everything sits under `ttl/30d/export_data/<user-uuid>/`.
- `prod-grok-backend.json` holds
  `{conversations[], projects[], tasks[], media_posts[]}`.
- `prod-mc-auth-mgmt-api.json` is the profile and sessions: personal data, not
  needed.
- `prod-mc-asset-server/<asset-uuid>/content` holds **uploaded file bytes**,
  with no extension; the type must be sniffed.
- Source: the Portable AI Memory mapping, "verified against observed exports,
  February 2026".

**Conversation:**
- `conversations[]` items are `{conversation, responses[{response, share_link}]}`.
- The conversation carries `id`, `title`, `create_time`/`modify_time` (ISO),
  `temporary`, `starred` and `system_prompt_name`.
- It may also carry `leaf_response_id`, which is rarely set (see Branches).

**Response:**
- `_id`, `message`, `sender`, `parent_response_id`, `create_time`, `model`.
- `sender` is `human`, `assistant`, `ASSISTANT`, or a model name.
- Reasoning: `thinking_trace`, `agent_thinking_traces[]`.
- Search: `web_search_results[{url, title, preview}]`, `xpost_ids`.
- Media: `generated_image_urls`, `file_attachments[asset ids]`.
- Tool steps: `steps[]`.

**Two eras** (June 2025, and October 2025 onwards):

| | June 2025 | October 2025 onwards |
|---|---|---|
| Conversation id | `conversation_id` | `id` |
| Response id | `response_id` | `_id` |
| Response time | ISO string | `{"$date":{"$numberLong":"<ms>"}}` |
| Model | `metadata.requestModelDetails.modelId` on the user turn | top-level `model` |

A reader takes both.

**Branches:**
- A regenerated reply is a sibling under the same parent.
- The branch last seen is `leaf_response_id` where it is set. It rarely is:
  one survey found it in 4 of 60 file variants.
- Otherwise take the newest leaf.
- `parent_response_id` is authoritative.

**Quirks:**
- Inline citation markup `<grok:render …>…</grok:render>` and `<xaiArtifact …>`
  tags sit inside `message`.
- Generated images are **URLs only**, most likely tied to a session.
- Files of 30–213 MB are reported.

**Projects** carry `custom_personality`, the instructions, but no chat→project
link was identified.

**Grok inside X is a different store.** The X archive's `data/grok-chat-item.js`
holds flat rows (`chatId`, `message`, `sender`, `createdAt`), with no parent
links, thoughts or attachments.

### 3.6 DeepSeek

**The ZIP:**
- `conversations.json` (an array) sits at the root.
- `user.json` holds the email and phone: personal data, not needed.

**Conversation:**
- Exactly `{id, title, inserted_at, updated_at, mapping}`.
- Times are ISO with microseconds and **`+08:00`** in every sample seen.

**`mapping`:**
- A `"root"` node plus numeric-string ids, each
  `{id, parent, children, message{model, inserted_at, fragments[]}}`.
- **There is no role field.** The role is told by fragment type.

| Fragment | Holds |
|---|---|
| `REQUEST` | The user's text |
| `RESPONSE` | The answer |
| `THINK` | Reasoning, **in full**. Often several per message, interleaved with tool steps |
| `TOOL_SEARCH` | `results[{title, url}]` |
| `SEARCH` | The legacy search block, `results[{title, url}]` |
| `TOOL_OPEN`, `TOOL_FIND` | Nothing: the page opened is not exported |
| `FILE` | `files[{file_id, file_name, file_size}]` |

**Branches:**
- There is **no current-branch pointer**, so take the newest leaf by
  `inserted_at` and walk it to the root.
- Start the walk at `"root"`, not at "the first node whose parent is root": the
  latter loses an edited first prompt.

**Files:** metadata only, with no text and no bytes.

**Traps:**
- The shape looks like ChatGPT's `mapping` with different names, so ChatGPT
  importers produce **empty chats without an error**. Recognize it positively.
- `results` is a JSON array in most readers. One census reports a Python-repr
  string.

**Scale seen:** 667 conversations in about 37 MB.

## 4. Prior art

The survey covered twelve clients and the standalone converters, checked on
2026-10-25.

**Few clients import at all.**
- **Open WebUI** imports ChatGPT.
- **LibreChat** imports ChatGPT, and Claude since 2025-12-30.
- **Cherry Studio** imports ChatGPT, and Claude since 2026-08-07.
- **TypingMind** (closed source) imports ChatGPT.
- **None imports Gemini Takeout or DeepSeek.** Grok exists only in LibreChat's
  unmerged draft
  ([PR #14505](https://github.com/LibreChat-AI/LibreChat/pull/14505)).
- **The rest import none of the five:** LobeHub, Chatbox, Jan, Msty,
  AnythingLLM, SillyTavern, big-AGI and Khoj. Several have open requests.
- **Google's Gemini itself** imports ChatGPT and Claude ZIPs since 2026-03-26
  ([9to5google](https://9to5google.com/2026/03/26/gemini-import/)):
  - text only, up to 5 GB each;
  - re-uploading an export overwrites the chats imported from it before;
  - not available in the EEA, the UK or Switzerland.

**Branches follow the data model.**
- **Tree-native clients keep every branch:** LibreChat, and Cherry Studio
  since its second importer.
- **Linear clients do one of two things.**
  - Some **flatten every node into one chain.** Open WebUI walks `mapping` in
    key order, so regenerations interleave with the answers they replaced.
  - Others **walk the current path:** Cherry Studio's first importer,
    sanand0/chatgpt-to-markdown.
- **mindfork is linear,** so the current path is its precedent. It is walked
  through `parent`.

**Nobody streams, and nobody reports what was lost.**
- Every merged client importer takes one JSON file and parses it whole. The
  caps: LibreChat 250 MiB, Cherry Studio 2 GB, Open WebUI none.
- No client importer opens the ZIP itself. Only converters do (convoviz,
  temnoon, Nexus, panchat), plus LibreChat's draft. They also read nested ZIPs
  and shards.

**The failures recur, and they are dated:**
- **Detection broken by a new kind of top-level entry:** Open WebUI
  [#23505](https://github.com/open-webui/open-webui/issues/23505), 2026-04.
- **Strict schemas refusing a missing field:**
  - LibreChat PR #13637, `metadata`, 2026-06;
  - convoviz #60, `status`/`weight`, 2026-07;
  - convoviz #70, a bare `{}`, 2026-10.
- **Silent loss:**
  - convoviz [#62](https://github.com/mohamed-chs/convoviz/issues/62)/#70
    rendered one message per chat once `children` went;
  - panchat once imported 100 of 1,285 conversations;
  - Cherry Studio PR #18135 dropped every human message of one shape.
- **False success:** LibreChat
  [#15229](https://github.com/LibreChat-AI/LibreChat/issues/15229), still open.
  A 50-shard export crashes the import, and the UI reports success.
- **Branch corruption and endless loops:** LibreChat #7003, #9491, #11123,
  #12313.
- **Invented model names:** Cherry Studio hard-codes `gpt-5` and
  `claude-sonnet-4-6`. Open WebUI fell back to `gpt-3.5-turbo` until 2026-05.
- **Leaked markup:**
  - ChatGPT's private-use citation markers, U+E200–E202. Only Cherry Studio
    cleans them.
  - Grok's `<grok:render>` and `<xaiArtifact>` tags.
- **A DeepSeek file routed to a ChatGPT reader** because both have `mapping`,
  and imported as nothing.

**The closest precedent in structure is
[modelcaddy/panchat](https://github.com/modelcaddy/panchat)** (Rust,
Apache-2.0).
- **What it reads:** ChatGPT, Claude and Gemini Takeout, into an interchange
  of its own.
- **What it keeps:** every branch plus an `active_path`.
- **What it reports:** a typed list of what was lost: `info`, `lossy`,
  `dropped`.
- That list is the shape of §6's preview.

**No interchange format has been adopted.**
- **The candidates:**
  - DTI's "AI Conversation": text, sender and time only.
  - Portable AI Memory v1.0: a DAG with thoughts, tool calls, the model, and
    files by reference. It has two commits and 12 stars.
  - panchat's "AI Chat Interchange 0.1".
- **The formats in actual use** are ChatGPT's raw `mapping`, SillyTavern JSONL
  and Markdown.
- **That answers F1 (c):** there is no standard worth reading instead of the
  sources.

**How often each format changed,** counted from parsers' fixes:

| Service | Changes |
|---|---|
| ChatGPT | About a dozen breaking changes over 2025–2026; in 2026, one every one to two months |
| Claude | About six over two years, plus the change of delivery |
| Grok, DeepSeek | Few |
| Gemini | The activity envelope is stable. What is inside it is not: verbs, record kinds, missing replies |

## 5. What mindfork can hold, and what that costs (the code as of 0.22.0)

| What the exports carry | Where it would go | What it costs |
|---|---|---|
| User and assistant text | `Message.text`, roles | — |
| Instructions: AI Studio's `systemInstruction`, ChatGPT's custom instructions | `Chat.system_message` | — |
| Thoughts | `Message.thoughts` | Display only: no request reads a stored message's thoughts (`orchestrator/request.rs`) |
| Model per message | `MessageMetadata.model` | **Seeds the profile's model history** from stored replies (`llm_history.rs::derive_llm_history`): imported `gpt-4o` replies become that profile's past models (spec §9.14) |
| Tool calls | `Message.tool_calls` plus tool messages | **Replayed to the model** as calls (`request.rs`, `assistant_tool_calls`). Imported calls would be calls of tools the request does not declare; what each wire does with that is unmeasured |
| Images | `Message.images`, base64 **inside the chat file** (spec §9.10) | Every chat file grows, and every chat file is parsed at startup (below) |
| Files | `Chat.files`, bytes in `data/files/<chat-id>/` | Listed, never sent. A new `FileOrigin` variant is a new value of a stored enum: a schema step (docs/lessons.md §8, `stored_enums_tests.rs`) |
| Extracted text of uploads (Claude) | `Chat.attachments` | **Re-injected into the system prompt on every turn** (spec §9.7) |
| Branches | Nothing: the history is linear. `deleted` holds removed exchanges | — |
| Timestamps | `Message.timestamp`, `Chat.created_at` / `modified_at` | — |

The neutral format holds less than the chat does: role, text, thoughts and a
timestamp per message (docs/import-format.md). It has no place for images,
files, the model or tool calls. A reader that goes through format v1 as it
stands loses what the chat could keep.

Four facts in the code decide more than any field:

1. **Every chat is parsed at startup.** `Orchestrator` loads every
   `chats/*.json` into `Vec<Chat>`, history included (`orchestrator/mod.rs`,
   `load_chats`); hidden chats are parsed too and then filtered out.
   - **Measured:** 36.5 ms in release for 14 MB (171 chats).
   - **The roadmap's own trigger:** the lazy orchestrator is deferred, to be
     revisited "at ~1000 chats, or a release parse over ~300 ms"
     ([roadmap.md](../roadmap.md)).
   - **This machine's dev data** is already 550 chats in 77 MB.
   - **One heavy ChatGPT account crosses that trigger.** 2,808 conversations and
     155 MB of JSON is about 400 ms by the same rate, before images. Images
     inline would multiply it.
   - **So the import may be what opens the lazy-orchestrator track,** and §8's
     probe measures whether it is.
2. **The search index is built from the same files.** At 10× the dev corpus a
   full rebuild was estimated at about 3.5 s
   ([chat-content-search.md](chat-content-search.md) §1.3). It is incremental
   afterwards.
3. **`import` reads the whole file into memory, capped at 64 MB**
   (`read_import_file`, the character card's cap). Exports run to hundreds of MB
   of JSON inside ZIPs of several GB.
   - **Streaming is needed:** a `serde_json` visitor over the array, one
     conversation at a time. No new crate is required.
   - **ZIP entries can be read** without unpacking them to disk.
4. **The self-model would reflect on another assistant's past.** An imported
   chat has `reflected_upto: None` (`features/import.rs`).
   - **The window becomes the whole history.** In a profile with the
     self-model tools, the first new turn's reflection window is the entire
     imported history (`reflection.rs::reflect_window`).
   - **The companion's memory picks it up too.** "Earlier conversations"
     (`companion_memory.rs`) lists the profile's chats, so imported ones would
     appear there.

**Already in the tree:**
- An HTML extractor (`extract_rich`, used by `fetch_url`, on `scraper`) that
  Gemini's HTML replies could pass through.
- A ZIP reader (`zip` 9, for backups).
- UUIDv5 idempotency by key (`features/import.rs`).

## 6. Design

The shape of the import, with the owner's decisions (§7) applied:

1. **`mindfork import <path>`, one command.** The path is an export ZIP, several
   ZIPs, a Claude manifest, an unpacked folder, or one JSON file. The source is
   recognized **by content**, as cards are today: ChatGPT by `mapping` +
   `current_node` or `author`; DeepSeek by `mapping` + `fragments` with no
   `author`; Claude by `chat_messages`; Grok by `conversations[].responses[]`;
   Gemini by activity records with a `gemini.google.com/app/` link; AI Studio by
   `chunkedPrompt`. A misrecognition is refused, never an empty import.
2. **A reader per source**, each writing one internal "import document". That
   document is the neutral format **plus compatible optional fields**
   (docs/import-format.md §Format evolution): a message's `model`, its images
   and files as references into the archive, the source and its conversation id.
   One writer turns it into chats. ADR 0007's single contract stays the
   contract. The readers are adapters onto it, built in.
3. **The branch the user last saw**, with a count of what was left out.
4. **What the app showed, not the plumbing.**
   - **Shown as text:** the answer, its sources as a short list, and code with
     its output as fenced blocks.
   - **Dropped and counted:** memory writes, hidden context and tool chatter.
5. **Images the export carries go inline,** as `MessageImage`s on their
   message, downscaled as an attached image is (`features::image_prepare`).
   This is the owner's F6 (b).
   - **Other files become the chat's files** on disk, with a placeholder line in
     the message.
   - **An image whose bytes the export lacks** becomes a placeholder line too.
6. **A preview before anything is written**: conversations, messages, branches
   dropped, files found and missing, nodes of kinds the reader did not know,
   date range. The import proceeds on a yes, or on `--yes`, as cards do.
7. **Tolerance as the rule.** Every field is optional. An unknown block with
   text becomes text, and an unknown block without text is counted. The
   preview's "unknown kinds" line is how format drift reaches the person (and a
   bug report) instead of a silent loss.
8. **Each conversation imports on its own,** and the summary says what failed.
   A run that read nothing is an error, never a success. This answers the false
   success and silent loss §4 found in four clients.

## 7. Forks

- **F1 — where the readers live.**
  - (a) **Built in**, as §6 describes. One command, no second tool. Each format
    change waits for a release.
  - (b) **External converters** emitting the neutral format, as ADR 0007 §3 is
    written: a script in `tools/` or a repository of its own. A fix ships
    without a release, but every user needs a second program (Python), and the
    format would still need extending for images and files.
  - (c) **An existing third-party converter** feeding our format. For example,
    a reader of Portable AI Memory (PAM) or panchat's interchange. No such
    format has been adopted, and each covers a subset of the five (§4).
  - The case for (a): the owner's F7 (a) for character cards already chose a
    built-in reader for a format "this widespread"
    ([companion-platform.md](companion-platform.md), C6). These five are
    widespread too; unlike cards they are undocumented and moving, which (b)
    answers better. Rule 7 of §6 is what makes (a) affordable.
  - **(a) is a decision ADR 0007 §3 did not make,** so it needs an ADR of its
    own.
  - **Recommended: (a). Owner's decision 2026-10-25: (a).** The ADR is written
    in stage 1.
- **F2 — which services, in what order.**
  - (a) **The two largest first:** ChatGPT and Claude in stage 1, DeepSeek and
    Grok (both small readers) in stage 2, Google (AI Studio, then Takeout) in
    stage 3.
  - (b) **The cleanest first:** DeepSeek, Grok and AI Studio prove the frame
    cheaply, then the two large ones.
  - (c) **Only what the probe's owner can export.**
  - The case for (a): if only one stage ever ships, it covers most of the
    audience. They are also the two that every importing client and Gemini's
    own import cover (§4). Takeout's activity log (no titles, no model,
    localized verbs) is the weakest data of the six and goes last.
  - **Recommended: (a). Owner's decision 2026-10-25: (a).**
- **F3 — which profile the chats land in.**
  - (a) **A new profile per source** ("ChatGPT", "Claude"…). Its system message
    is empty, and it has no self-model tools and no companion switches.
  - (b) **A profile the person names** (`--profile <name>`).
  - (c) **A profile per custom GPT, project or Gem.** Mostly impossible: Claude
    and Grok exports do not link chats to projects, and ChatGPT's carry project
    ids without names.
  - **Recommended: (a) by default, with (b) as a flag. Owner's decision
    2026-10-25: (a) with (b) as a flag.**
- **F4 — branches.**
  - (a) **Only the branch last seen**, with the count of the others in the
    preview.
  - (b) Also every other branch, **as a chat of its own** ("title — branch 2").
  - (c) Other branches **into `deleted`**, the removed-exchange list. That gives
    the list a meaning it does not have, and reflection reads its causes.
  - Every linear precedent takes the current path. The flattening alternative
    is Open WebUI's interleaving, the defect the survey found most often (§4).
  - **Recommended: (a), with (b) behind a flag in a later stage if asked for.
    Owner's decision 2026-10-25: (a).**
- **F5 — tool activity.**
  - (a) **As text**: what the app showed (§6, item 4).
  - (b) **As `ToolCallRecord`s.** The feed's tool view would show them, but
    they are replayed to the model as calls of tools it does not have (§5).
  - (c) **Dropped.**
  - **Recommended: (a). Owner's decision 2026-10-25: (a).**
- **F6 — images and files.**
  - (a) **The chat's files** on disk (a new `FileOrigin::Imported`, so a schema
    step), with a placeholder line in the message. Claude's extracted text also
    becomes a file, not an attachment (§5).
  - (b) **Images as `MessageImage`s**, downscaled and inline. The model sees
    them when the chat is continued, but every chat file and the startup parse
    grow.
  - (c) **Names only.**
  - **Recommended: (a). Owner's decision 2026-10-25: (b) for images, (a) for
    every other file**, asked separately. What follows from that:
    - **Images are kept, not sent, depending on their message.** An image on a
      user message is replayed to a model that sees images. An image on an
      assistant reply — the images ChatGPT and Grok generated — is kept and
      shown but never sent: `request.rs` sends images on user and tool
      messages only. It costs file size, and nothing in the request.
    - **A model without vision is already handled.** Images are withheld, with
      a note by name (`request.rs::withhold_images`,
      [history-images-no-vision.md](history-images-no-vision.md)).
    - **The startup parse grows by the images' bytes** (§5, fact 1). With
      F9 (a) nothing bounds it, so the probe measures it with the images in
      (§8).
- **F7 — the self-model and the companion.**
  - (a) An imported chat is **marked as already reflected on**: the watermark is
    set at its end, so the self-model never learns from another assistant's
    past. In a profile of its own (F3 a) it never reaches a companion's
    "earlier conversations" either.
  - (b) **Treated as any chat.**
  - **Recommended: (a). Owner's decision 2026-10-25: (b).** What follows from
    that:
    - **No reflection by default.** The default profile per source (F3 a) has no
      self-model tools, so reflection never runs there. It runs where
      `--profile` puts the chats into a profile that has them.
    - **Reflection waits for a new turn.** It is checked when a turn completes
      in that chat (`reflection.rs::maybe_auto_reflect`), so an imported chat
      nobody continues is never read.
    - **The first reflection's window is the whole imported history.** The
      digest is capped: past its budget it keeps the start and the end and
      drops the middle (`rename_chat.rs::build_conversation_digest`,
      `truncate_middle`). The request cannot overflow, but the middle of a long
      import is not read.
    - **The companion's memory sees imported chats too.** In such a profile,
      "earlier conversations" (`companion_memory.rs`) lists them as that
      profile's past.
- **F8 — importing again.** A newer export of the same account lands on the
  same chats (UUIDv5 of source + conversation id).
  - (a) **Overwrite.** This is what the neutral import does today, and it would
    erase what was said in mindfork after the import.
  - (b) **Overwrite a chat untouched since the import; skip a continued one,**
    and say so in the preview. This needs an "imported from" stamp on the chat:
    a new optional field holding source, id, source time and the message count
    at import.
  - (c) **Only new conversations.**
  - **Recommended: (b). Owner's decision 2026-10-25: (b).**
- **F9 — a very large account.**
  - (a) **Import everything** and accept the startup cost the probe measures.
  - (b) **Filters:** `--since DATE`, `--limit N`.
  - (c) **Make the orchestrator lazy first,** the deferred roadmap track.
  - **Recommended: (a) + (b), with (c) opened only if the probe's
    heavy-account import costs more than about a second at startup in release.
    Owner's decision 2026-10-25: (a)** — everything is imported, with no
    `--since` and no `--limit`, and the startup cost is accepted as it comes.
    - **The probe still measures the cost** (§8), images included (F6 b).
    - **The number goes to the roadmap's lazy-orchestrator item,** which stays
      deferred and keeps its own trigger.

Not forks, decided by the shape above:
- **Personal files are never read.** That means `user.json`,
  `prod-mc-auth-mgmt-api.json`, `users.json` and `ads.json`.
- **Nothing is fetched.** Grok's image URLs and Gemini's links stay text.
- **HTML is converted, never rendered.**
- **Every tree walk is iterative, with a cycle guard.**
- **Entry names are never used as paths:** files are written under our own
  names, as stored files are today (`entities/chat_file.rs`).

## 8. Stages and the probe

**Stage 0 — the probe** (`spike/cloud-chat-import-probe`).
- **The exports:** the owner requests one from each of the five services, and
  has an account with each. For Google that means both Takeout's Gemini Apps
  activity, as JSON, and the AI Studio folder. The agent cannot request them:
  each request is an action on the owner's account.
- **The reader:** a throwaway one per source, counting:
  - conversations, and messages on the chosen branch;
  - branches left out;
  - node or block kinds it did not know;
  - thoughts present;
  - files found and files referenced but missing;
  - models.
- **The startup measurement:** the release startup parse with the whole import
  in a copy of the data, against the same copy without it.
  - **Images inline (F6 b):** the import is measured with its images inline,
    along with the chat files' total size.
  - **The record:** the number goes to the roadmap's lazy-orchestrator item
    (F9 a).
- **GO, per source**:
  - ≥ 99% of conversations read without error;
  - for five conversations chosen by the owner, the imported branch matches what
    the service's app shows;
  - one imported chat continued live, on Gemma 4 and on one cloud model, answers
    with the imported context.
- **On a NO-GO:** a source is dropped from the track, or kept as text-only.

**Stage 1 — the frame, ChatGPT and Claude** (F2 a):
- recognition, the import document, the preview, the dedicated profile and
  `--profile`;
- the branch, thoughts, model names, tool activity as text;
- images inline, and other files as the chat's files;
- re-import protection;
- the ADR.

Two changes to stored data come with stage 1:
- **`FileOrigin::Imported`** is a new value of a stored enum. It needs a schema
  step and the previous release's refusal (docs/lessons.md §8,
  `stored_enums_tests.rs`).
- **The chat's "imported from" stamp** (F8 b) is a new optional field. It is
  additive under ADR 0006.

**Stage 2 — DeepSeek and Grok.**

**Stage 3 — AI Studio, then Gemini Takeout.**

Tests are fixtures written from the probe's redacted real files: one per era of
each format (ChatGPT with and without `children`; Grok's two eras; DeepSeek with
`SEARCH` and with `TOOL_*`). The live run is the probe's continued-chat check,
repeated at each stage.

## 9. Not in this track

- **Memory.** ChatGPT's memory snapshot, Claude's `memories` part. Another
  assistant's notes about the person could become notes, but the self-model is
  the companion's own, and that is a question for the owner after this track.
- **Exporting back** to any service.
- **Grok inside X** (`grok-chat-item.js`): a different file and store.
- **Organization exports**: Claude Enterprise, Gemini via Vault, ChatGPT's
  Compliance API.
- **Claude Code.** Its local transcripts are not a cloud export. Its cloud
  sessions do come inside Claude's export, as `claude_code_cloud-000/`, but in a
  format of their own.
- **Browser extensions and live-API capture** (§1).

## 10. Sources

Research of 2026-10-25, through web search and fetching of current code and
pages. The ones the claims above rest on:

- **ChatGPT**
  - Help pages (fetched through search digests; direct fetches got 403):
    - [help.openai.com/en/articles/7260999](https://help.openai.com/en/articles/7260999)
    - [help.openai.com/en/articles/9106926](https://help.openai.com/en/articles/9106926-transfer-exported-conversations-between-chatgpt-accounts)
  - Parsers and schema notes:
    - [convoviz](https://github.com/mohamed-chs/convoviz)
    - [temnoon/openai_export_parser](https://github.com/temnoon/openai_export_parser)
    - [Gusarovv/chatgpt-export-viewer](https://github.com/Gusarovv/chatgpt-export-viewer)
    - [slyubarskiy/chatgpt-conversation-extractor SCHEMA_NOTES](https://github.com/slyubarskiy/chatgpt-conversation-extractor/blob/master/docs/SCHEMA_NOTES.md)
    - [pionxzh/chatgpt-exporter](https://github.com/pionxzh/chatgpt-exporter)
    - [LibreChat's fixture](https://github.com/danny-avila/LibreChat/tree/main/api/server/utils/import/__data__)
  - Real samples and community reports:
    - the redacted October 2026 sample in [marelucent/chatgpt-export-reader#3](https://github.com/marelucent/chatgpt-export-reader/issues/3)
    - [the OpenAI community thread on the 2025 change](https://community.openai.com/t/chatgpt-export-data-organization-has-changed-again-with-no-documentation/1161967)
    - [ChatKeeper's release notes](https://martiansoftware.com/chatkeeper/releases/index.html)
- **Claude**
  - Help pages:
    - [support.claude.com 9450526](https://support.claude.com/en/articles/9450526-export-your-claude-data)
    - [the organization export](https://support.claude.com/articles/13346720)
    - [Anthropic's import page](https://claude.com/docs/third-party/claude-desktop/import.md)
  - [Nexus AI Chat Importer](https://github.com/Superkikim/nexus-ai-chat-importer), with its issues #63, #65, #72 and #73. The August 2026 redacted sample is attached to #73.
  - The manifest in [tokpop.dev#36](https://github.com/Gaurav890/tokpop.dev/issues/36).
  - Readers:
    - [osteele/claude-chat-viewer](https://github.com/osteele/claude-chat-viewer)
    - [AlanTuring01/claude-export-reader](https://github.com/AlanTuring01/claude-export-reader)
    - [simonw/claude-to-sqlite](https://github.com/simonw/claude-to-sqlite) (the 2024 shape)
- **Gemini and AI Studio**
  - Help pages:
    - [support.google.com/gemini/answer/16920332](https://support.google.com/gemini/answer/16920332)
    - [answer/13278892](https://support.google.com/gemini/answer/13278892)
  - The [My Activity schema](https://developers.google.com/data-portability/schema-reference/my_activity).
  - Readers:
    - [Sinity/polylogue](https://github.com/Sinity/polylogue): Rust, `gemini_app.rs` and `ai_studio.rs`
    - [OTLFrostA/gemini-exporter](https://github.com/OTLFrostA/gemini-exporter)
    - [baip/chatbots-export](https://github.com/baip/chatbots-export)
    - [modelcaddy/panchat](https://github.com/modelcaddy/panchat)
- **Grok**
  - The [Portable AI Memory mapping](https://portable-ai-memory.org/providers/grok/) and its [converter](https://github.com/portable-ai-memory/python-sdk).
  - The June 2025 sample and schema in [yetanotherchris/openwebui-importer](https://github.com/yetanotherchris/openwebui-importer).
  - [The xAI FAQ](https://docs.x.ai/grok/faq).
  - [Dicklesworthstone/xf](https://github.com/Dicklesworthstone/xf), for the X archive.
- **DeepSeek**
  - The [privacy policy, zh-CN, 2026-02-10](https://cdn.deepseek.com/policies/zh-CN/deepseek-privacy-policy.html), for the UI path.
  - Censuses of real exports:
    - [SunshineMahiru/deepseek-ucs-adapter](https://github.com/SunshineMahiru/deepseek-ucs-adapter)
    - [Chily201/deepseek](https://github.com/Chily201/deepseek)
    - [liwangzimo/organize-deepseek-conversations](https://github.com/liwangzimo/organize-deepseek-conversations)
  - [The knopki gist](https://gist.github.com/knopki/ea43aa2a04171f1a9e3e3639c8744ed0).
  - [urumchi/deepseek-keeper](https://github.com/urumchi/deepseek-keeper).
- **Prior art**
  - Client importers:
    - Open WebUI's [`src/lib/utils/index.ts`](https://github.com/open-webui/open-webui/blob/main/src/lib/utils/index.ts)
    - LibreChat's [`importers.js`](https://github.com/danny-avila/LibreChat/blob/main/api/server/utils/import/importers.js), and its draft [PR #14505](https://github.com/LibreChat-AI/LibreChat/pull/14505)
    - Cherry Studio's [`importers/`](https://github.com/CherryHQ/cherry-studio/tree/main/src/renderer/services/import/importers)
  - Converters:
    - [modelcaddy/panchat](https://github.com/modelcaddy/panchat), with its [SPEC](https://github.com/modelcaddy/panchat/blob/main/SPEC.md)
    - [sanand0/chatgpt-to-markdown](https://github.com/sanand0/chatgpt-to-markdown)
  - Gemini's own import: [9to5google, 2026-03-26](https://9to5google.com/2026/03/26/gemini-import/).
  - [The PAM conversation schema](https://portable-ai-memory.org/schema/conversations/).
