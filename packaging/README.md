# Install Traversiq

This archive contains a native Rust binary, Codex and Claude marketplace metadata, MCP wiring, setup/impact skills, and pre-edit hooks. It is for the platform named in the archive filename. Keep the extracted directory available while its local marketplace is registered. No Python or Rust toolchain is needed to run the packaged plugin.

## Download installer

Once a public release is available, download `install.sh` from the [latest release](https://github.com/OsnielT/Traversiq/releases/latest), inspect it if desired, and run `sh install.sh codex` or `sh install.sh claude`. It selects the native package, checks its SHA-256 digest, extracts it to a versioned directory under `~/.local/share/traversiq` (or `$XDG_DATA_HOME/traversiq`), and runs the same host commands shown below. No `sudo`, Rust, or Python is needed. Downloads use GitHub HTTPS; checksums detect damaged downloads, not a compromised publisher.

Rerunning for the same version reuses its directory without overwriting files. To switch to a newer release, first remove the previous local marketplace with `codex plugin marketplace remove traversiq-local` or `claude plugin marketplace remove traversiq-local`, then run the new installer and restart. Claude removes that marketplace's installed plugins as well. Workspace configuration remains outside the plugin directory. To uninstall, remove the marketplace from the host before deleting its downloaded directory. If host registration fails, the installer keeps the downloaded directory and prints its path so you can retry the commands below.

Linux packages are built on Ubuntu 22.04 and require glibc 2.35 or newer; Alpine/musl Linux is not supported. macOS packages are built on macOS 15. Intel and ARM64 builds must pass their release jobs before publication. Windows and release signing are not included.

## Manual installation

Download the archive matching your platform and its `.zip.sha256` file from the same release. Verify it with `shasum -a 256 -c <archive>.zip.sha256` (macOS) or `sha256sum -c <archive>.zip.sha256` (Linux). Unzip the archive, then run these commands from its extracted top-level directory for the host you use:

```sh
# Codex
codex plugin marketplace add .
codex plugin add traversiq@traversiq-local

# Claude Code
claude plugin marketplace add .
claude plugin install traversiq@traversiq-local
```

These are local/manual marketplace installs. Hook-bearing Codex packages are not currently eligible for the public plugin directory. See the [OpenAI packaging documentation](https://developers.openai.com/plugins/build/plugins) and [Claude marketplaces documentation](https://code.claude.com/docs/en/plugin-marketplaces).

Restart the host and ask Traversiq to set up the current project. In Claude, you can invoke `/traversiq:setup`. Use your existing developer-confirmed workspace/local JSON files, or use the setup skill to propose and refine the project graph. Configuration belongs outside the installed plugin cache.

For manual setup, run the packaged binary from each intended working project:

```sh
/absolute/path/to/extracted/plugins/traversiq/bin/traversiq plugin configure /absolute/path/to/workspace.json /absolute/path/to/local.json
```

This validates the existing graph and creates `.traversiq/config.json` in the current directory, recording the two absolute config paths. Existing pointers are never overwritten. Add `.traversiq/` to that project's `.gitignore`. To change mappings later, edit the pointer or its referenced files. Relative pointer paths resolve from the pointer's own directory. A pointer can also be placed in an intentionally chosen common parent; the nearest pointer wins for projects below it.

Restart the MCP connection after setup or changing the pointer. Use the Traversiq workspace tool to confirm the selected projects. In Codex, review and trust the bundled hook through `/hooks`; installing a plugin does not trust its hooks. Keep normal host permissions in place. Replace duplicate manual Traversiq hooks/MCP registrations when adopting the plugin.

Until a pointer exists, the plugin hook reports that impact is unchecked and does not deny edits. Its MCP server reports a setup error. Once configured, Codex `apply_patch` and Claude `Edit`/`Write` use the existing checkpoint: first attempt returns partial impact evidence, and an identical retry within 120 seconds proceeds only if freshly computed evidence matches in the same session and turn/prompt. Claude must supply `prompt_id` (verified with 2.1.287); Codex 0.163.0 was verified. Receipts and content caches stay beside the referenced local config.

Evidence is partial. Checkpoint failures, untrusted/disabled hooks, shell writes, notebook edits, MCP writers and other tool paths can leave edits unchecked. A retry does not prove that a model understood the briefing. Graph membership grants no permission to edit another project. The engine reads approved code; it does not execute application build scripts or send it to an additional model service. Your coding host still controls how retrieved evidence reaches its model.

The original engine is Apache-2.0 licensed. The plugin directory includes LICENSE, NOTICE, THIRD_PARTY_NOTICES.md and PROVENANCE.md. Source and build instructions: [Traversiq repository](https://github.com/OsnielT/Traversiq). Public release signing and Windows packages are not included in this first package.

## Build and publish

From a source checkout, run `python3 scripts/build-plugin.py` with Python 3.11+ and Rust installed. It builds a native, path-remapped binary and creates an allowlisted archive plus checksum under `target/plugins/`. Local research, workspace mappings and application source are excluded.

The GitHub Actions release workflow tests and packages macOS ARM64/Intel and Linux ARM64/x86-64 on native runners. A manual workflow run builds downloadable CI artifacts without publishing. Pushing a tag matching `Cargo.toml`, both plugin manifests and the installer version (for example, `v0.1.0`) creates a **draft** GitHub release only after every build passes. Review its four archives, four checksum files, installer and generated notes, then publish the draft. The README download URL becomes usable after the first non-prerelease release is published. The workflow does not submit Traversiq to a public plugin directory.
