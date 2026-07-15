# Assistant

oproxy includes an assistant surface for chat-driven inspection and configuration.

The assistant is designed as a control-plane client. It reads current oproxy state through allowlisted tools and prepares changes as confirmation cards.

## Provider Setup

Open the `Assistant` and enter:

- Provider base URL, for example `https://api.openai.com/v1`
- For Ollama on the host while oproxy runs in Docker, use `http://host.docker.internal:11434`; oproxy normalizes this to `/v1/chat/completions`
- Model name
- API key, unless the provider is a local no-auth provider such as Ollama

The key is stored in browser `sessionStorage` for the current tab/session and is sent only with `/admin/assistant/chat`. oproxy does not persist the key to `storage_path`.

Any OpenAI-compatible provider that supports chat completions and function-style tool calls can be used.

## What It Can Do

Read-only work can run automatically:

- Read the product feature catalog to choose the right oproxy capability
- Search and summarize sessions
- Inspect one session
- Read config, throttling, DNS, capture filter, upstream proxy, webhooks, mocks, scripts, breakpoints, and rules

Changes require confirmation:

- Create, update, or delete rules
- Change throttling, DNS, capture filter, upstream proxy, mocks, scripts, webhooks, or breakpoints
- Replay traffic, clear sessions, or send an outbound forward request

When the assistant proposes a change, review the action card and click `Apply` to execute it.

## Capability Architecture

The assistant is intentionally structured as a control-plane client rather than an autonomous backdoor.

- `src/control_plane/assistant_capabilities.yaml` is the declarative source of truth for assistant-facing product knowledge.
- The capability manifest teaches the model what oproxy can do, where to do it in the UI, when to use each feature, which read tools expose state, which admin endpoints apply changes, and the OpenAI-compatible tool schemas.
- `src/control_plane/assistant_registry.rs` owns manifest loading, feature catalog lookup, grouped tool metadata, OpenAI tool specs, and workspace-action tool name mapping.
- `src/control_plane/assistant_contracts.rs` owns typed execution contracts for model-visible tools: execution kind, risk, confirmation requirement, and affected resources.
- For general how-to/setup questions, the assistant should answer with UI steps first, then mention assistant automation or API details.
- `src/control_plane/assistant_context.rs` owns the assistant-visible backend context envelope. It combines workspace state, visible Sessions summaries, selected session summary, and redacted browser hints.
- `src/control_plane/assistant_action_contracts.rs` owns confirmed admin-action endpoint contracts: allowed path shape, supported methods, risk classification, action kind, and affected resources.
- `src/control_plane/assistant_payload_repair.rs` owns conservative model-output repair for known scalar fields, such as numeric and boolean strings, before domain validation.
- `src/control_plane/assistant_actions.rs` owns proposal defaults, payload shape validation, and confirmed action execution through existing control-plane services.
- `src/control_plane/assistant_tools.rs` owns allowlisted tool dispatch and read-tool implementations. It delegates workspace UI tools to workspace actions and proposal tools to assistant actions.
- `src/control_plane/assistant_provider.rs` owns OpenAI-compatible provider config, URL normalization, local no-auth provider handling, request dispatch, and response parsing.
- `src/control_plane/assistant_prompt.rs` owns system prompt construction and redacted provider message history.
- `src/control_plane/assistant_redaction.rs` owns assistant-specific value/string redaction shared by context, prompt, tools, actions, and provider errors.
- Backend-owned workspace state is authoritative for what the user is looking at: active surface, Sessions filters, selected session, sort, view mode, visible results, and feature view hints.
- Browser-sent `client_context` is treated only as ephemeral hints. It must not be used as the source of truth for product or workspace state.
- The Rust tool contract tests verify that every model-exposed tool has a typed execution contract and that every contract is exposed intentionally.
- The Rust action contract tests verify that confirmed admin actions are allowlisted by typed endpoint contracts rather than ad hoc route checks.
- `GET /admin/assistant/tools` returns grouped tool metadata enriched with `execution_kind`, `requires_confirmation`, `risk`, and `refreshed_resources` so the UI can explain what an assistant tool is allowed to do.
- Payload repair normalizes common model output mistakes into the same scalar shapes used by the UI, while domain modules still perform final Rust type validation.
- The executor validates payloads against the same Rust data models before running existing control-plane logic.
- Confirmation tokens bind the reviewed action to the exact server-side payload that will execute.

This keeps product knowledge maintainable: new oproxy features should add or update the manifest entry first, expose a read/proposal handler only when new behavior is actually executable, and route execution through existing admin/API behavior.

## Security Model

- API keys are never written to oproxy storage.
- Sensitive keys such as `authorization`, `cookie`, `token`, `secret`, and `api_key` are redacted before context is sent to a model.
- The model can call only server-side allowlisted tools.
- Mutating, destructive, replay, and network actions are stored server-side with short-lived confirmation tokens.
- Existing admin auth, CSRF checks, and admin egress policy still apply.

## Limitations

- The assistant currently supports OpenAI-compatible chat and tool-calling APIs.
- Server-side key vault and fully automatic mutation are not included.
- If a provider does not support tool calls, the assistant can still answer plain chat but cannot reliably inspect or propose oproxy actions.
