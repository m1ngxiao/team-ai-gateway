# Daily Codex model synchronization

This optional Linux task checks the latest stable `@openai/codex` release once
per day and adds verified public models to an existing Team AI Gateway. It runs
outside the gateway container, using administrator RPC for model changes and a
read-only SQLite connection for probe-key identity checks. Python 3.10 or newer
and systemd are sufficient; no Python packages or Codex installation are needed.

## What a run does

1. Read the official npm `latest` release and the bundled model catalog from its
   matching `openai/codex` `rust-v<version>` tag. Only models with
   `visibility=list` and `supported_in_api=true` are candidates. Pre-releases,
   an empty catalog, duplicate slugs, and changed catalog schemas are rejected.
2. In apply mode, update `gatewayUserAgentVersion` through RPC when necessary.
   Query the gateway using the configured probe key and require a recent genuine
   official account catalog for that exact client version. Refresh the catalogs
   of all active OpenAI keys using account rotation through authenticated APIs.
3. Skip every model already present in the managed catalog, including disabled,
   hidden, edited, or custom-priced entries. Existing routes and prices remain
   as configured. Models missing from the new CLI catalog are retained in the
   gateway; they are absent from the latest CLI export.
4. For each new model, require an official standard price, including every
   published short/long-context tier. The pricing HTML and its referenced
   same-host official JS asset are parsed as data; JavaScript is never executed.
   An unknown price or threshold leaves the model pending for a later run.
5. Verify the requested model with streaming text, a forced streaming function
   call, and a streaming continuation containing the tool result. The response
   model must match, the stream must start and complete, the tool arguments must
   be correct, and the final output must be `OK`. These small requests consume
   real account usage. Failed models stay pending and are not imported.
6. Run an existing-model streaming canary, recheck concurrent additions, preview
   the import, and commit with `conflictStrategy=keep_existing`. New models use an
   `account_pool/default` route, their original upstream slug, passthrough
   instructions, and the confirmed official prices. No existing key is rebound.
7. Publish the latest source catalog, verified CLI export, and success report as
   one snapshot using an atomic `current` symlink switch. A failed run retains
   the previously published snapshot; `last-run.json` records its failure.

If a client-version change fails, the task restores the previous version and
its previously saved genuine official cache files, retaining each file's
original owner, group, and mode before publishing its atomic replacement.
This rollback requires the
gateway RPC and host filesystem to remain available. New models committed before
a subsequent publication failure remain in the gateway; retrying is additive
and preserves them. The model database transaction and filesystem publication
are separate operations, not one distributed transaction.

## Configure a server

Use a private configuration directory outside the Git checkout. In the examples
below, `/srv/team-ai-gateway` is the checked-out repository and
`/etc/team-ai-gateway-model-sync` is the operator-selected configuration directory.
Choose paths that fit your deployment.

```bash
sudo install -d -m 700 /etc/team-ai-gateway-model-sync
sudo cp /srv/team-ai-gateway/deploy/model-sync/config.example.json /etc/team-ai-gateway-model-sync/config.json
sudo cp /srv/team-ai-gateway/deploy/model-sync/probe-key.example.json /etc/team-ai-gateway-model-sync/probe-key.json
sudo chmod 600 /etc/team-ai-gateway-model-sync/config.json /etc/team-ai-gateway-model-sync/probe-key.json
```

Edit `config.json` and the private `probe-key.json` before running:

| Option | Meaning |
| --- | --- |
| `gateway_url` | Gateway root URL serving `/rpc` and `/v1/*`; HTTPS or loopback HTTP. |
| `database` | Existing gateway SQLite database on the host, including its readable WAL files. |
| `rpc_token_file` | Existing administrator RPC token file. |
| `probe_key_file` | Private JSON with `id` and `key` for an active OpenAI platform key using account rotation. The key must match the database. |
| `official_cache_dir` | Existing `official-model-catalogs` directory in the gateway data volume, accessible on the host. |
| `state_dir` | Private directory for the process lock, snapshots, and operational reports. |
| `outbound_proxy` | Optional HTTP(S) proxy for npm, GitHub source, and official pricing; `null` means direct access. Gateway RPC/API calls bypass proxies. |
| `canary_model` | Optional preferred existing model; falls back to a current authorized, enabled model if unavailable. |
| `probe_key_name` | Optional extra identity check against the key's display name. Usually omit it. |

Relative filesystem paths are resolved beside `config.json`, independently of
the current working directory. The example `gateway-data` path is a placeholder:
point it at your existing host-mounted gateway data directory. The scheduler
account needs read access to the database, RPC token, configuration, and probe
key. Create `state_dir` with the scheduler account's ownership and private
permissions; that directory contains scheduler-owned operational state.

`official_cache_dir` is shared gateway state. Retain the gateway's existing
ownership and its read/write access to that directory and its files; do not
change the shared cache's ownership to root or the scheduler account. The
scheduler also needs permission to read and replace cache files during rollback.
For a non-root scheduler, grant the necessary directory/file access using ACLs
or a suitable shared group. Access alone does not permit restoring another
user's ownership: run under the gateway's UID or provide a narrowly scoped
privileged restoration mechanism when cache ownership differs. If original
ownership cannot be restored, replacement fails before publishing a cache with
incorrect permissions.
Credential files and operational state must not be committed to Git.

Inspect a dry run before enabling the timer:

```bash
sudo python3 /srv/team-ai-gateway/scripts/model-sync/model_sync.py --config /etc/team-ai-gateway-model-sync/config.json
```

The default dry run writes only a local run report. It can refresh the gateway's
normal `/v1/models` cache but does not update its client version, change managed
models, or consume generation requests. If the client version needs updating,
it reports that prerequisite. `--smoke-new` also tests new candidates during a
dry run, when the current client version already matches. Use `--apply` to run
the verified additive update and publish a snapshot:

```bash
sudo python3 /srv/team-ai-gateway/scripts/model-sync/model_sync.py --config /etc/team-ai-gateway-model-sync/config.json --apply
```

The task requires RPC methods `appSettings/get`, `appSettings/set`,
`apikey/managedModelListV2`, `apikey/managedModelGetV2`,
`apikey/managedModelImportPreviewV2`, and `apikey/managedModelImportCommitV2`, plus
the gateway's per-key genuine official catalog cache. Deployments that lack
these interfaces must add compatible support before enabling this task.

The portable Compose template also sets the gateway's existing client-version
metadata polling interval to 86400 seconds. This poll does not add managed
models. On other deployment templates, set
`CODEXMANAGER_CODEX_LATEST_SYNC_INTERVAL_SECS=86400` and restart the gateway to
use the same interval; the service source default without that setting is six
hours. The daily model timer is installed independently below.

## Install a daily systemd timer

Render units using your actual paths and scheduler account. The renderer writes
only the specified output directory; it does not install units or contact a
gateway. The default schedule is 03:00 UTC daily, with up to five minutes of
random delay. `--calendar` accepts a systemd `OnCalendar` expression, including a
timezone, for example `*-*-* 03:00:00 Asia/Shanghai`.

```bash
python3 /srv/team-ai-gateway/scripts/model-sync/render_units.py --config /etc/team-ai-gateway-model-sync/config.json --user root --output ./rendered-model-sync
```

For a configuration directory readable only by root, run the renderer with
`sudo` as well. Review the generated `.service` and `.timer` files, then install:

```bash
sudo systemd-analyze verify ./rendered-model-sync/team-ai-gateway-model-sync.service ./rendered-model-sync/team-ai-gateway-model-sync.timer
sudo install -m 644 ./rendered-model-sync/team-ai-gateway-model-sync.service ./rendered-model-sync/team-ai-gateway-model-sync.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now team-ai-gateway-model-sync.timer
systemctl list-timers team-ai-gateway-model-sync.timer
```

`Persistent=true` runs a missed daily check after the server returns. The service
uses a private umask, a 20-minute timeout, a read-only system filesystem, and write
access limited to the configured state and official-cache directories. It does
not restart the gateway or alter proxy selection. A manual service run uses the
same configuration:

```bash
sudo systemctl start team-ai-gateway-model-sync.service
journalctl -u team-ai-gateway-model-sync.service -n 50 --no-pager
```

Inspect `state_dir/last-run.json` for `added`, `pending`, `existing_preserved`, and
verification results. `state_dir/last-success.json` follows the current published
snapshot. Standard output contains operational metadata only, without API keys,
raw HTTP bodies, or model instructions. Snapshots are retained; operators can
archive older snapshot directories while keeping the one referenced by `current`.

Disable scheduling without deleting any models:

```bash
sudo systemctl disable --now team-ai-gateway-model-sync.timer
```

## Concurrency and client catalogs

The file lock serializes updater processes sharing a `state_dir`. A second model
list check after slow probes and `keep_existing` preserve models added before
the import's conflict check. The current server RPC checks conflicts before its
upsert transaction; there is a small remaining window in which an independent
administrator can insert the same previously absent slug before the write.
Avoid concurrent manual edits/imports for the same new slug, and have all
scheduled updaters for one gateway share the same state directory. This task
does not claim a server-side atomic "insert only if absent" guarantee.

The verified export is `state_dir/gateway-models.json`. It contains public Codex
metadata, including official instructions, and no account credentials. It is an
operator export, not a replacement for per-key API authorization. An existing
custom Codex `model_catalog_json` file must be updated or replaced explicitly;
the task cannot modify colleagues' local machines. Codex loads that configured
file at startup, so restart the CLI/app after distributing an updated catalog.
See [official gateway rollout guidance](https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway).

## Independent tests

All fixtures are synthetic. Tests require no keys, containers, live network,
local server, or host-specific downloaded HTML:

```bash
python3 -m unittest discover -s scripts/model-sync -p 'test_*.py' -v
python3 scripts/model-sync/model_sync.py --help
python3 scripts/model-sync/render_units.py --help
```
