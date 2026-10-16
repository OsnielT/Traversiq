# Install Traversiq

This archive contains a native Rust binary, Codex and Claude marketplace metadata, MCP wiring, setup/impact skills, and pre-edit hooks. It is for the platform named in the archive filename. Keep the extracted directory available while its local marketplace is registered. No Python or Rust toolchain is needed to run the packaged plugin.

Unzip the archive, then run these commands from its extracted top-level directory for the host you use:

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
