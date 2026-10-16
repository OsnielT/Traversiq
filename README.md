# Traversiq

A Rust CLI and local MCP server for investigating how a change could affect connected projects. The CLI, Rust crate and MCP server are named `traversiq`.

The engine reads JavaScript/TypeScript imports and exports, GraphQL schemas and static queries, and Go resolver-method syntax. Reports include source locations, coverage limits and unresolved candidates. Candidates are not proven breakages, and an empty result does not establish safety.

## Try the synthetic example

The four projects in `examples/demo` are purpose-built examples: a shared UI library, two consumers, and an API. They require no application dependency installation, credentials, network services or build scripts. The source is intended for static analysis, not a running application.

```sh
cargo test --locked
cargo run -- validate examples/demo/workspace.json examples/demo/local.json
cargo run -- impact examples/demo/workspace.json examples/demo/local.json ui
cargo run -- trace examples/demo/workspace.json examples/demo/local.json ui src/index.ts Button
cargo run -- api-trace examples/demo/workspace.json examples/demo/local.json api schema.graphql Query.catalog.summary
```

The UI trace should find the `web` and `admin` consumers. The API trace should find the web query, its service/route imports, and two Go method-name candidates.

## Connect your projects

Use `examples/workspace.json` and `examples/local.example.json` as configuration templates. `web.depends_on: ["ui"]` means web consumes ui. Relative project paths resolve against the local config file. Confirm suggested relationships before adopting them; discovery does not prove installed package versions or deployed API bindings.

```sh
cargo run -- discover /path/to/projects
cargo run -- validate /path/to/workspace.json /path/to/local.json
```

Discovery reads bounded package.json manifests and proposes relationships with evidence. It does not change the scanned projects. Config validation rejects unknown fields, missing/duplicate roots, unknown dependencies and self-dependencies. Dependency cycles are supported.

## Install the plugin

The first package supports local/manual marketplace installation in Codex and Claude Code. It bundles the native Rust engine, MCP server wiring, pre-edit hooks, and setup/impact skills. Runtime use needs neither Python nor a Rust compiler. Build tooling requires Python 3.11+ and Rust:

```sh
python3 scripts/build-plugin.py
```

The builder compiles the host platform's release binary with remapped source paths, then creates `target/plugins/traversiq-<version>-<target>.zip` and a SHA-256 checksum. Its explicit file list excludes application configs, caches, private research and the docs folder. Keep the extracted directory available while its marketplace is registered. Other platforms need their own builds; Windows packaging and release signing are outside this first package.

Follow the [installation guide](packaging/README.md) to register the extracted marketplace and install `traversiq@traversiq-local` in the host you use. Hook-bearing Codex packages currently use local/manual distribution and are not eligible for the public plugin directory; see the [official packaging guidance](https://developers.openai.com/plugins/build/plugins).

Ask Traversiq to set up the working project, or use `/traversiq:setup` in Claude. The bundled binary's `plugin configure <workspace.json> <local.json>` command validates developer-confirmed mappings and creates `.traversiq/config.json` in the current directory without overwriting existing pointers. Keep that machine-local directory out of version control. The plugin finds the nearest pointer in the working directory or its ancestors; relative pointer paths resolve from the pointer's directory. Restart the MCP connection after setup or changing the pointer, and confirm the workspace through its MCP tool. Codex hook trust remains a separate host review through `/hooks`.

Before setup, plugin hooks report that impact is unchecked and do not deny edits, while the MCP server reports a setup error. Once configured, supported edits use the checkpoint described below. A malformed nearer pointer fails clearly instead of falling back to a different workspace. Project mappings are stored outside the plugin cache and survive replacing the package. Replace duplicate manual MCP/hook registrations if you adopt the plugin; the existing CLI and manual templates remain available.

## MCP access

```sh
cargo build --locked --release
target/release/traversiq mcp examples/demo/workspace.json examples/demo/local.json
```

The server waits for protocol messages on stdin. Configure an MCP host with that launch command rather than typing chat into the process. It exposes `traversiq_workspace`, `traversiq_impact`, `traversiq_trace`, and `traversiq_api_trace` using the same engine as the CLI. Use the [Codex template](examples/codex-mcp.toml) or [Claude template](examples/claude-mcp.json). Replace template absolute paths with your release binary and workspace/local JSON files. For Codex, merge its template into the working project's `.codex/config.toml`; for Claude, merge its template into `.mcp.json`. Rebuild the binary and restart the host after changing the setup. Host trust and approval remain controlled by the host.

Start with `traversiq_workspace` to get project IDs. Use `traversiq_impact` with a `project` ID, `traversiq_trace` with `provider`, relative `entry`, and `symbol`, or `traversiq_api_trace` with `provider`, relative `schema_entry`, and `field_path`. Trace tools accept optional `all_warnings`. To try the demo, ask the host to trace `Button` from provider `ui`, entry `src/index.ts`.

One analysis runs per server process; overlapping calls return a busy error. Cancellation does not forcibly stop the blocking parser. Tool results provide evidence and do not grant editing permission.

## Pre-edit checkpoints (experimental)

The standalone adapters use `traversiq pre-edit <workspace.json> <local.json>` with the host's JSON event on stdin. Build the release binary before configuring either hook. These standalone templates are alternatives to the plugin's bundled hook; use one registration per host.

For Codex, merge the [Codex hook template](examples/codex-hooks.toml) into your approved working project's `.codex/config.toml`. Replace the binary and both configuration paths, then review and trust the exact hook with `/hooks`. An untrusted hook does not run. This adapter follows the [official OpenAI hook contract](https://learn.chatgpt.com/docs/hooks) and handles `apply_patch`.

For Claude Code, merge the [Claude hook template](examples/claude-hooks.json) into the working project's `.claude/settings.local.json`, replacing all three paths. Keep that machine-local file out of version control. Review the loaded hook with `/hooks` and restart the session after changing its configuration. This adapter follows [Claude's hook contract](https://code.claude.com/docs/en/hooks) and handles `Edit` and `Write`, including new files and overwrites. It requires the host's `prompt_id`; older hosts without it receive a blocking incomplete result rather than falling back to a session-wide receipt. Claude Code 2.1.287 was verified. Neither adapter returns an `allow` decision or changes native permission settings. Do not install duplicate copies of a hook.

The checkpoint checks proposed source/destination paths against approved project roots, then reports declared project consumers, unverified package-name import candidates (including subpaths), and transitive JS/TS file-import candidates across configured projects. Package-name candidates do not prove which installed package or edited symbol a consumer uses. The first attempt is denied with a partial-coverage briefing. The agent should review it and investigate relevant warnings. An identical retry within 120 seconds proceeds through normal host authorization only if recomputed evidence matches. Receipts are scoped to the tool, session, Codex turn or Claude prompt, and agent ID when provided. A different tool-call ID is required to acknowledge the earlier attempt, and the receipt is consumed once. A retry is an opportunity for review, not proof that a model understood the report.

Changed proposals, indexed JS/TS bodies, touched files, configuration or scopes require another briefing. Unindexed files and API/Go contents outside touched targets are outside the freshness check. Additions and unindexed files receive project-level evidence only. Parent traversal and symlink paths are rejected; use absolute approved paths for sibling projects. Schema-field selection, Go calls and documentation relationships still need targeted investigation with the existing source/API tools. Results remain partial, including when no candidates are found.

Analysis has a five-second budget. Handled errors and timeouts emit a blocking incomplete result without issuing a receipt. Fix the reported problem, or deliberately disable the hook through the host if you choose to proceed without it. An absent binary, skipped hook or host-side hook failure may let the host continue. Shell writes, notebook edits, MCP writers, other tools and hosts without this configured hook can bypass it. This is a workflow guardrail, not a filesystem security boundary.

The hook leaves applications unchanged and writes only the existing source cache and a private `*.traversiq-checkpoint.json` receipt beside the local config. The receipt holds hashes, a tool-call ID and a timestamp; keep it local. One pending proposal is retained per local config, so interleaved sessions may need another briefing. Reads and freshness checks are not an atomic snapshot. Checkpoint output grants no permission to edit another project.

## Scope and limitations

Source scans cover regular JS/TS files under configured `src` directories and non-test Go files for the API provider. Relative ESM modules and simple JSON tsconfig aliases are supported. Parser errors retain only unaffected top-level facts and are reported. CommonJS, dynamic imports, full TypeScript checking, Go receiver-type resolution and runtime behavior are not resolved.

API analysis matches a selected schema field to static gql templates, same-document fragments and named query imports. Dynamic/interpolated or escaped documents are skipped. Endpoint binding is unverified. Schema aggregation, cross-document fragments, directives and full GraphQL validation remain outside this slice.

Scans are bounded at 10,000 files/directories per project, depth 20 and 1 MiB per file. Result groups cap at 100; warnings cap at 50 unless `--all-warnings` is requested. Downstream traversal has its own documented bounds. Reports retain omitted counts and uncertainty.

Scanned applications remain read-only. Trace commands write a content-hashed SQLite cache beside the local config. Caches contain paths and extracted source facts, including query literals; keep them private. Files are read and hashed each request, and relationships are rebuilt from current facts. This is not an atomic filesystem snapshot. Each language scan updates the cache in a transaction. A locked, corrupt or unwritable cache fails the command clearly rather than returning an old report; the SQLite lock wait is 100 ms. Close running scans before removing a disposable cache to rebuild it.

## Development

Run `cargo test --locked`, `cargo fmt --check`, and `cargo clippy --locked --all-targets -- -D warnings`. The test suite uses synthetic temporary workspaces and exercises configuration, discovery, source/API tracing, invalidation and MCP behavior. No complete impact accuracy or token savings are claimed.

For an opt-in runtime check, build with `cargo build --locked` and run `python3 scripts/smoke-codex-hook.py` (or pass a binary path). This was verified with Codex 0.163.0. It uses a loopback mock model and temporary demo files to assert that the first patch is blocked, the briefing reaches the model, and an identical retry edits the file. It trusts only the generated fixture hook for that invocation, keeps normal sandbox permissions, disables apps/plugins, and uses a temporary model catalog. It does not test real-model reasoning or every Codex surface.

Run `python3 scripts/smoke-claude-hook.py` (or pass a binary path) for the Claude runtime check. It uses a loopback mock API, a disposable Claude configuration directory and temporary demo files. Only Read/Edit/Write are available, with ordinary edits authorized through `acceptEdits`; no permission bypass is used. It asserts that Edit and Write are denied before changing files, both briefings reach the model, and identical retries succeed. No real model is called. This check does not measure model understanding or verify IDE/desktop behavior.

After building a plugin archive, run `python3 scripts/test-plugin.py` for archive allowlist, executable-bit, checksum and version/build-path checks. To exercise an extracted plugin in an isolated Claude session, use `python3 scripts/smoke-claude-hook.py --plugin /path/to/extracted/plugins/traversiq`; it also verifies the packaged MCP workspace tool. Package skills and metadata were read through Codex's read-only plugin API, but a complete Codex marketplace installation was not performed. Runtime checks use mock responses and do not prove model understanding. No global host configuration is changed by the checks.

Run `python3 scripts/update-licenses.py --check` to check generated notices and `python3 scripts/check-release.py` to inspect Cargo's public package file list. The latter performs a limited content check; it is not a comprehensive secret scan or legal audit.

[Third-party notices](THIRD_PARTY_NOTICES.md) retain dependency licensing and attribution. [Provenance](PROVENANCE.md) records the implementation's relationship to reference projects. Original contributions are licensed under [Apache-2.0](LICENSE). Cargo registry publication is disabled with `publish = false`; this does not affect building from this repository. Private trial material is retained locally and excluded from the source package.

Broader write-tool coverage, automatic documentation updates, and contradiction detection remain future work.
