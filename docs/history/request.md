# Console AI chat application in Rust. Name — mindfork-rs.

> 🗄 **Archived document.** This is the project's original brief — the starting
> point, fixed before implementation. Some decisions have since changed: the
> `xinfer` engine was replaced with **llama.cpp `llama-server`**, and the
> `ratatui-textarea`/`ratatui-markdown` UI crates with custom widgets (see
> [ADR 0001](../decisions/0001-ui-crates-ratatui-030.md),
> [ADR 0003](../decisions/0003-own-markdown-renderer.md)). Current state — in
> [README.md](../../README.md), [CLAUDE.md](../../CLAUDE.md) and [spec.md](../../spec.md).


- Platform support — Windows, Linux.

- Architectural approach — your choice, but I think Feature-Sliced Design fits
  better for apps built with AI agents.

- TUI library — ratatui, ratatui-textarea — for the user's message input and
  editing already-sent messages. Also need spellcheck for Russian and English.

- Markdown widgets — ratatui-markdown. Need support for everything LLMs output
  as Markdown, plus a unicode approximation of LaTeX so arrows and simple math
  formulas look readable. No need to render to an image, since this app might
  run in a JupyterLab terminal. Mermaid diagram rendering isn't needed for now
  either.

- The app needs a chat list, opened by a button and a hotkey, with the ability
  to rename chats. Also need an input field to search for a chat by filtering
  the list on entered text in the title. And two sort modes for the list — by
  creation date and by last-modified date.

- Messages need collapsible CoT blocks and tool-call blocks.

- Need the ability to edit existing messages, both user's and the AI's.

- Need a button to delete the Assistant's last message. In this case, the
  user's last message is also deleted, and its text moves into the user
  message input field. If the input field isn't empty, the deleted message's
  text is prepended to the existing text.

- Also need a button to regenerate the Assistant's last message.

- LLM integration should be implemented with the lightweight xinfer library
 (https://github.com/guoqingbao/xinfer).

- The app needs AI-persona profiles — a unique identifier, a system message,
  and an optional greeting message (some models behave more interestingly if
  the AI assistant starts the conversation first).

- The AI assistant should be able to use tools: RAG, the ability to save and
  retrieve notes, use Python with libraries, use web search (duckduckgo), and
  more. The assistant should get to know the user better through conversation
  and store information about them. The assistant should also be able to get
  the date and time of the user's last message, get sampling parameters,
  change sampling parameters, get its own system message in the current chat
  and change it, at will. This might take the liveliness of the conversation
  to a new level. Notes and RAG must be stored separately per AI-persona
  identifier, to avoid mixing information.

- Model support — Gemma 3, 4 and Qwen 3.5, 3.6 are mandatory. Other models
  only if xinfer's functionality allows it effortlessly. I want Gemma and
  Qwen to become much smarter, more interesting, and more self-aware than
  they are now, even with CoT blocks. I've thought hard about how to do this.
  I looked into multi-agent response generation, but the result didn't
  satisfy me. It seems the model needs a tool that lets it spin up another
  agent for a short time — something like: a call_subagent(system_message,
  message) function that returns the subagent's answer as a string. Calling
  the subagent wouldn't differ from an ordinary tool call, but letting the
  model set its own system message might well have a positive effect on the
  answer. Same goes for the ability to change its own system message.