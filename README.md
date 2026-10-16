# Traversiq

Understand what a code change could affect before you edit.

Traversiq helps Codex and Claude Code investigate connected projects—frontend apps, shared libraries and APIs—using a local Rust engine. It returns source references, possible downstream effects and gaps in the analysis.

For example: changing a shared `Button` can reveal its importing apps and downstream files. Changing a GraphQL field can reveal static queries and possible Go resolver methods.

**Early alpha:** results are evidence to investigate, not proof that a change will break something or that it is safe.

## What it does

- Maps the projects you choose and their dependencies.
- Traces JavaScript/TypeScript imports and exports across projects.
- Connects selected GraphQL fields to static queries and Go method candidates.
- Provides automatic checkpoints before supported edits in Codex and Claude Code.

Use it as a plugin or directly through the CLI. The plugin includes a local MCP server, which lets your coding agent query the engine, plus setup and impact-analysis skills.

## Get started

From a source checkout, build the plugin package:

```sh
python3 scripts/build-plugin.py
```

Building requires **Python 3.11+ and Rust**, including Cargo, Rust's build tool. Running the packaged plugin requires neither.

The archive and its SHA-256 checksum appear in `target/plugins/`. Extract the archive and follow the [installation guide](packaging/README.md) for your host.

After installation:

1. Ask Traversiq to set up your project. In Claude, use `/traversiq:setup`.
2. Choose the related projects and confirm or refine their suggested dependencies.
3. Restart the host/MCP connection and confirm the selected workspace. In Codex, review and trust the hook through `/hooks`.

Setup stores your configuration in `.traversiq/config.json`, outside the plugin cache. Keep it out of version control. Existing configuration is never overwritten by the setup command.

**Before setup, edits remain unchecked.** The plugin reports that configuration is missing; it does not block edits.

## Try the example

The four projects in [examples/demo](examples/demo) contain a shared UI library, two consumers and an API. No application dependencies, credentials or running services are needed.

With Rust and Cargo installed, run:

```sh
# Find consumers of the shared Button export
cargo run -- trace examples/demo/workspace.json examples/demo/local.json ui src/index.ts Button

# Investigate a GraphQL field across the API and its client
cargo run -- api-trace examples/demo/workspace.json examples/demo/local.json api schema.graphql Query.catalog.summary
```

The UI trace finds the `web` and `admin` consumers. The API trace finds a web query, its service/route imports and two Go method candidates.

For your own projects, start with the [workspace](examples/workspace.json) and [local paths](examples/local.example.json) templates. `web.depends_on: ["ui"]` means web consumes ui. Discovery can suggest relationships for you to review:

```sh
cargo run -- discover /path/to/projects
cargo run -- validate /path/to/workspace.json /path/to/local.json
```

## How the checkpoint works

Once configured, Traversiq intercepts Codex `apply_patch` and Claude `Edit`/`Write` calls:

1. The first attempt pauses the edit and returns an impact briefing.
2. The agent reviews the evidence and investigates relevant warnings.
3. An identical retry within 120 seconds can proceed if fresh analysis still matches, subject to normal host permissions.

Changed proposals or relevant evidence require a new briefing. Analysis has a five-second budget; handled errors and timeouts return a blocking, incomplete result. A retry gives the agent an opportunity to review—it does not prove understanding.

The automatic checkpoint covers project dependencies and partial JS/TS file relationships. API and Go investigation still uses the targeted analysis tools.

## Current limits

- Analysis is static and partial. Dynamic imports, full type checking, deployed API bindings and runtime behavior are not resolved. An empty report does not establish safety.
- Shell writes, notebook edits, MCP writers and other tool paths can bypass the checkpoint. Disabled, untrusted or failed hooks can leave edits unchecked.
- Analysis leaves application source files unchanged. Local caches contain paths and extracted source facts, including query text; keep them private.
- The built archive targets the machine that builds it. The macOS Apple Silicon package was tested; other platforms need separate builds. Windows packaging and release signing are not included.
- Claude's extracted plugin was tested with a mock model. Codex package discovery and checkpoint behavior were checked, but a full Codex plugin installation remains untested.

Automatic documentation updates and contradiction detection are planned, not implemented. No measured token savings or complete impact accuracy are claimed.

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

The tests use synthetic workspaces. After building the plugin, run `python3 scripts/test-plugin.py` for archive checks. The [Codex](scripts/smoke-codex-hook.py) and [Claude](scripts/smoke-claude-hook.py) smoke checks exercise host behavior with local mock responses, without paid model calls.

<details>
<summary>Advanced configuration and analysis details</summary>

The plugin finds the nearest `.traversiq/config.json` in the working directory or its ancestors. Relative paths resolve from that file's directory. A malformed nearer configuration stops discovery. Restart the MCP connection after changing which workspace/local files it points to.

For a manual connection, use the [Codex MCP template](examples/codex-mcp.toml) or [Claude MCP template](examples/claude-mcp.json). Standalone [Codex hooks](examples/codex-hooks.toml) and [Claude hooks](examples/claude-hooks.json) are also available. Replace their paths and use one registration per host to avoid duplicating the plugin's wiring.

The MCP server exposes `traversiq_workspace`, `traversiq_impact`, `traversiq_trace` and `traversiq_api_trace`. Tool schemas describe their arguments. One analysis runs per server process; overlapping calls return a busy error, and cancellation does not forcibly stop a blocking parser.

Source scans cover regular JS/TS files under configured `src` directories. They support relative ESM modules and simple JSON tsconfig aliases. API tracing matches selected schema fields to static gql templates, same-document fragments and named query imports; Go candidates use non-test method syntax. Dynamic/interpolated queries, schema aggregation and Go receiver types are not resolved.

Scans are bounded at 10,000 files/directories per project, depth 20 and 1 MiB per file. Result groups cap at 100 and warnings at 50; trace commands accept `--all-warnings`. Reports retain omitted counts and coverage limits.

Trace commands write a content-hashed SQLite cache beside the local config. Reads are not an atomic filesystem snapshot. Locked, corrupt or unwritable caches fail clearly instead of returning old reports; close running scans before removing a disposable cache to rebuild it.

Checkpoint receipts are single-use and scoped to the exact tool input, session, turn/prompt and agent ID when provided. Only one pending proposal is retained per local config. Unindexed files and API/Go content outside touched targets are outside the freshness check. New files receive project-level evidence only. Parent traversal and symlink edit paths are rejected. Claude must supply `prompt_id`; older hosts without it receive an incomplete result.

Use `python3 scripts/update-licenses.py --check` to check dependency notices and `python3 scripts/check-release.py` to inspect the public source-package file list. The release check is limited; it is not a comprehensive secret scan or legal audit.

</details>

## License

Original contributions are licensed under [Apache-2.0](LICENSE). See [third-party notices](THIRD_PARTY_NOTICES.md) for dependency attribution and [provenance](PROVENANCE.md) for the relationship to reference projects.
