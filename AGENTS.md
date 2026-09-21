# Team AI Gateway engineering guide

The source in this repository is maintained directly. Do not fetch and patch an external checkout to make a build work.

- `apps/`: static Next.js management UI; read `apps/AGENTS.md` before changes. Tauri sources are retained for compatibility; current supported releases target Linux/Web.
- `crates/core/`: storage and migrations; `crates/rusqlite/`: local SQLite compatibility layer; `crates/service/`: API gateway, routing and authentication; `crates/web/`: UI host; `crates/start/`: launcher.
- `services/dashboard/`: optional read-only collector and usage dashboard.
- `deploy/`: portable deployment templates. `scripts/`: deployment and maintenance tools. `docs/`: public project documentation.

Keep changes focused. Preserve API/RPC names, database schema compatibility and authentication boundaries. Never add accounts, credentials, databases, private infrastructure or host-specific configuration to the repository. Use example values and existing configuration mechanisms.

Gateway changes need meaningful stream/non-stream and retry/cancellation regressions. UI changes need runtime tests and a static production build. Dashboard changes need Python and frontend tests. Deployment changes need deployment tests and Compose validation. Run the narrow checks during development, then the documented complete checks before publication. Do not silently skip failing tests.

Keep the original MIT attribution and upstream provenance. Update user-facing docs when behavior, supported deployment targets or configuration changes.
