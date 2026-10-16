# Implementation provenance

This repository contains a Rust implementation of workspace validation, package discovery, bounded source extraction, impact retrieval, local caching and an MCP adapter. Third-party dependencies and their notices are listed in THIRD_PARTY_NOTICES.md and licenses/inventory.json.

Graphify was inspected as a reference during early research. The current Cargo dependency graph does not include Graphify, and the engine does not invoke or bundle its Python implementation or skill. Early research and application-specific evaluation material are retained locally outside the public package. No clean-room development claim is made: source was inspected, and a full provenance/similarity review has not been completed.

Ponytail instructions guided the development workflow. The plugin and skill text are not bundled in the public implementation. Future copying or bundling requires a review of the exact material and its license, with applicable notices retained. Using development guidance does not make it a runtime dependency.

The MCP adapter uses rmcp and the SDK's documented APIs. Its full upstream license includes legacy MIT notices in addition to Apache terms; these are preserved in the third-party notices. Parser grammars, Unicode data licensing and bundled/native components require their own applicable notices.

The public demo is newly written synthetic static-analysis input. It does not require or reproduce an application checkout. Documentation does not include private trial reports, local absolute paths, or screenshots/Word exports of those reports.

Reference-project names identify research provenance and do not imply affiliation or endorsement. The developer selected Apache-2.0 for original contributions; see LICENSE. Traversiq is the selected product name. Original contributions are attributed to Osniel Torres in NOTICE. Repository publication was authorized by the project owner. Name clearance and a full source-provenance review have not been completed. Permission to analyze local projects did not establish permission to publish their contents.
