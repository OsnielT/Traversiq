---
name: setup
description: Set up Traversiq in the current project using developer-confirmed workspace and local project mappings, or diagnose its workspace configuration.
---

Use the bundled `bin/traversiq`, located two directories above this skill directory. Resolve its actual installed path before executing it; the plugin cache is read-only application content.

Use existing workspace/local JSON files when available. Otherwise, identify the projects the developer wants analyzed. The binary's `discover <approved-projects-directory>` command suggests package relationships; confirm or refine the suggested graph with the developer before adopting it. Do not scan unrelated directories or infer permission to edit connected projects.

From the intended working project, run the bundled binary with `plugin configure <workspace.json> <local.json>`. This validates the mappings and creates `.traversiq/config.json` with machine-local paths. It never overwrites an existing pointer. If a pointer exists, inspect it and update only the configuration the developer requested. Relative paths in this file resolve from its own directory. Keep it out of version control.

The plugin searches the current directory and its ancestors for the nearest `.traversiq/config.json`. Configure each working project separately, or put a shared pointer in an intentionally selected common parent. A malformed nearer pointer stops discovery instead of falling back to another workspace.

Restart the host/MCP connection after setup or changing the pointer. Codex requires review and trust of the bundled hook through `/hooks`; keep native tool permissions in place. Remove duplicate manual Traversiq MCP/hook registrations if the developer is replacing them with this plugin. Check the configured workspace through the Traversiq MCP tool before reporting setup complete.

Until configuration exists, plugin hooks report that impact is unchecked and do not deny edits; the MCP server reports a setup error. Once configured, supported edits use the pre-edit checkpoint. Hook failures or unsupported writing tools can bypass that checkpoint. Setup does not prove that a change is safe.
