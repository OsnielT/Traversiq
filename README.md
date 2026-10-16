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

## MCP access

```sh
cargo build --locked --release
target/release/traversiq mcp examples/demo/workspace.json examples/demo/local.json
```

The server waits for protocol messages on stdin. Configure an MCP host with that launch command rather than typing chat into the process. It exposes `traversiq_workspace`, `traversiq_impact`, `traversiq_trace`, and `traversiq_api_trace` using the same engine as the CLI. Use the [Codex template](examples/codex-mcp.toml) or [Claude template](examples/claude-mcp.json). Replace template absolute paths with your release binary and workspace/local JSON files. For Codex, merge its template into the working project's `.codex/config.toml`; for Claude, merge its template into `.mcp.json`. Rebuild the binary and restart the host after changing the setup. Host trust and approval remain controlled by the host.

Start with `traversiq_workspace` to get project IDs. Use `traversiq_impact` with a `project` ID, `traversiq_trace` with `provider`, relative `entry`, and `symbol`, or `traversiq_api_trace` with `provider`, relative `schema_entry`, and `field_path`. Trace tools accept optional `all_warnings`. To try the demo, ask the host to trace `Button` from provider `ui`, entry `src/index.ts`.

One analysis runs per server process; overlapping calls return a busy error. Cancellation does not forcibly stop the blocking parser. Tool results provide evidence and do not grant editing permission.

## Scope and limitations

Source scans cover regular JS/TS files under configured `src` directories and non-test Go files for the API provider. Relative ESM modules and simple JSON tsconfig aliases are supported. Parser errors retain only unaffected top-level facts and are reported. CommonJS, dynamic imports, full TypeScript checking, Go receiver-type resolution and runtime behavior are not resolved.

API analysis matches a selected schema field to static gql templates, same-document fragments and named query imports. Dynamic/interpolated or escaped documents are skipped. Endpoint binding is unverified. Schema aggregation, cross-document fragments, directives and full GraphQL validation remain outside this slice.

Scans are bounded at 10,000 files/directories per project, depth 20 and 1 MiB per file. Result groups cap at 100; warnings cap at 50 unless `--all-warnings` is requested. Downstream traversal has its own documented bounds. Reports retain omitted counts and uncertainty.

Scanned applications remain read-only. Trace commands write a content-hashed SQLite cache beside the local config. Caches contain paths and extracted source facts, including query literals; keep them private. Files are read and hashed each request, and relationships are rebuilt from current facts. This is not an atomic filesystem snapshot. Each language scan updates the cache in a transaction. A locked, corrupt or unwritable cache fails the command clearly rather than returning an old report; the SQLite lock wait is 100 ms. Close running scans before removing a disposable cache to rebuild it.

## Development

Run `cargo test --locked`, `cargo fmt --check`, and `cargo clippy --locked --all-targets -- -D warnings`. The test suite uses synthetic temporary workspaces and exercises configuration, discovery, source/API tracing, invalidation and MCP behavior. No complete impact accuracy or token savings are claimed.

Run `python3 scripts/update-licenses.py --check` to check generated notices and `python3 scripts/check-release.py` to inspect Cargo's public package file list. The latter performs a limited content check; it is not a comprehensive secret scan or legal audit.

[Third-party notices](THIRD_PARTY_NOTICES.md) retain dependency licensing and attribution. [Provenance](PROVENANCE.md) records the implementation's relationship to reference projects. Original contributions are licensed under [Apache-2.0](LICENSE). Cargo registry publication is disabled with `publish = false`; this does not affect building from this repository. Private trial material is retained locally and excluded from the source package.

Installable plugin packaging, automatic pre-edit enforcement, automatic documentation updates, and contradiction detection remain future work.
