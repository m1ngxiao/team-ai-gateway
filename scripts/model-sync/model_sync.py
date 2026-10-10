#!/usr/bin/env python3
"""Daily, additive synchronization of the stable Codex catalog into Team Gateway."""
import argparse
import concurrent.futures
import datetime as dt
import fcntl
import hashlib
import json
import os
import re
import shutil
import sqlite3
import stat
import sys
import tempfile
import time
import urllib.error
import urllib.request
from urllib.parse import urlsplit
from pathlib import Path

REGISTRY = 'https://registry.npmjs.org/@openai%2Fcodex/latest'
PRICING = 'https://developers.openai.com/api/docs/pricing'
MAX_DOCUMENT = 12 * 1024 * 1024
SLUG = re.compile(r'[a-z0-9][a-z0-9._-]{0,100}\Z')
STABLE = re.compile(r'\d+\.\d+\.\d+\Z')


class SyncError(Exception):
    """A safe, operator-readable failure; never include upstream response bodies."""


class SameOriginRedirect(urllib.request.HTTPRedirectHandler):
    """Keep credentials and official-source reads on their original HTTPS host."""
    def redirect_request(self, request, fp, code, message, headers, new_url):
        old, new = urlsplit(request.full_url), urlsplit(new_url)
        if (old.scheme, old.netloc) != (new.scheme, new.netloc):
            raise SyncError('cross_origin_redirect_rejected')
        return super().redirect_request(request, fp, code, message, headers, new_url)


def make_opener(proxy=None):
    return urllib.request.build_opener(
        urllib.request.ProxyHandler({'http': proxy, 'https': proxy} if proxy else {}), SameOriginRedirect())


PATH_FIELDS = ('database', 'rpc_token_file', 'probe_key_file', 'official_cache_dir', 'state_dir')


def load_config(path):
    """Validate operator settings; resolve paths beside the configuration file."""
    path = Path(path).resolve()
    try:
        config = json.loads(path.read_text())
    except (OSError, ValueError):
        raise SyncError('configuration_unreadable') from None
    if not isinstance(config, dict):
        raise SyncError('configuration_must_be_an_object')
    allowed = set(PATH_FIELDS) | {'gateway_url', 'outbound_proxy', 'canary_model', 'probe_key_name'}
    if set(config) - allowed:
        raise SyncError('unknown_configuration_option')
    for name in PATH_FIELDS:
        value = config.get(name)
        if not isinstance(value, str) or not value.strip():
            raise SyncError('configuration_path_required:' + name)
        target = Path(value).expanduser()
        config[name] = str((path.parent / target).resolve())
    gateway = config.get('gateway_url')
    if not isinstance(gateway, str):
        raise SyncError('gateway_url_required')
    try:
        parsed = urlsplit(gateway)
        local_http = parsed.scheme == 'http' and parsed.hostname in ('127.0.0.1', 'localhost', '::1')
        if not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment or not (local_http or parsed.scheme == 'https'):
            raise ValueError
        parsed.port
    except ValueError:
        raise SyncError('gateway_url_must_be_https_or_loopback_http') from None
    config['gateway_url'] = gateway.rstrip('/')
    proxy = config.get('outbound_proxy')
    if proxy is not None:
        if not isinstance(proxy, str):
            raise SyncError('invalid_outbound_proxy')
        try:
            parsed_proxy = urlsplit(proxy)
            if parsed_proxy.scheme not in ('http', 'https') or not parsed_proxy.hostname or parsed_proxy.path not in ('', '/') or parsed_proxy.query or parsed_proxy.fragment:
                raise ValueError
            parsed_proxy.port
        except ValueError:
            raise SyncError('invalid_outbound_proxy') from None
    canary = config.get('canary_model')
    if canary is not None and (not isinstance(canary, str) or not SLUG.fullmatch(canary)):
        raise SyncError('invalid_canary_model')
    if 'probe_key_name' in config and not isinstance(config['probe_key_name'], str):
        raise SyncError('invalid_probe_key_name')
    return config


def utc_now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def snapshot_timestamp(value):
    if type(value) in (int, float):
        return value
    if isinstance(value, str):
        return dt.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()
    raise ValueError('invalid snapshot timestamp')


def atomic_write(path, data, *, metadata=None):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name + '-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as target:
            target.write(data)
            target.flush()
            if metadata is not None:
                current = os.fstat(target.fileno())
                if (current.st_uid, current.st_gid) != (metadata['uid'], metadata['gid']):
                    os.fchown(target.fileno(), metadata['uid'], metadata['gid'])
                # chown may clear mode bits, so restore mode after ownership.
                os.fchmod(target.fileno(), metadata['mode'])
            os.fsync(target.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def write_json(path, value):
    atomic_write(path, (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode())


def publish_snapshot(state_dir, catalog, export, report):
    """Publish the three documents together with one final atomic pointer switch."""
    if not public_models(export):
        raise SyncError('empty_verified_export')
    snapshots = state_dir / 'snapshots'
    snapshots.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = Path(tempfile.mkdtemp(prefix='.preparing-', dir=snapshots))
    names = ('gateway-models.json', 'source-catalog.json', 'last-success.json')
    link_path = state_dir / ('.current-' + temporary.name)
    created_aliases = []
    try:
        for name, value in zip(names, (export, catalog, report)):
            write_json(temporary / name, value)
        final = snapshots / ('v' + report['cli_version'] + '-' + temporary.name.removeprefix('.preparing-'))
        os.rename(temporary, final)
        for name in names:
            alias = state_dir / name
            target = 'current/' + name
            if alias.is_symlink():
                if os.readlink(alias) != target:
                    raise SyncError('unexpected_catalog_alias')
            elif alias.exists():
                raise SyncError('existing_catalog_file_requires_migration')
            else:
                os.symlink(target, alias)
                created_aliases.append(alias)
        os.symlink('snapshots/' + final.name, link_path)
        os.replace(link_path, state_dir / 'current')
    except Exception:
        for alias in created_aliases:
            alias.unlink(missing_ok=True)
        raise
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)
        link_path.unlink(missing_ok=True)


def fetch(opener, url, *, data=None, headers=None, timeout=50):
    headers = dict(headers or {})
    headers.setdefault('User-Agent', 'TeamGateway-CodexCatalogSync/1.0')
    if data is not None:
        headers['Content-Type'] = 'application/json'
        data = json.dumps(data).encode()
    request = urllib.request.Request(url, data=data, headers=headers)
    try:
        with opener.open(request, timeout=timeout) as response:
            raw = response.read(MAX_DOCUMENT + 1)
            if len(raw) > MAX_DOCUMENT:
                raise SyncError('document_too_large')
            return raw, response.headers.get('Content-Type', '')
    except urllib.error.HTTPError as error:
        raise SyncError('http_' + str(error.code)) from None
    except (urllib.error.URLError, TimeoutError, OSError):
        raise SyncError('network_request_failed') from None


def validate_catalog(value):
    if not isinstance(value, dict) or not isinstance(value.get('models'), list):
        raise SyncError('catalog_schema_changed')
    rows = value['models']
    if not 1 <= len(rows) <= 250:
        raise SyncError('invalid_catalog_size')
    names = set()
    for row in rows:
        if not isinstance(row, dict) or not SLUG.fullmatch(str(row.get('slug', ''))):
            raise SyncError('invalid_model_slug')
        if row['slug'] in names:
            raise SyncError('duplicate_model_slug')
        names.add(row['slug'])
        if type(row.get('supported_in_api')) is not bool or row.get('visibility') not in ('list', 'hide', 'hidden'):
            raise SyncError('catalog_flags_changed')
        if not isinstance(row.get('display_name'), str):
            raise SyncError('catalog_metadata_changed')
    return rows


def public_models(catalog):
    return [row for row in validate_catalog(catalog)
            if row['supported_in_api'] and row['visibility'] == 'list']


def read_sources(opener):
    raw, _ = fetch(opener, REGISTRY)
    release = json.loads(raw)
    if not isinstance(release, dict):
        raise SyncError('registry_release_schema_changed')
    version = release.get('version', '')
    if not isinstance(version, str) or not STABLE.fullmatch(version) or release.get('name') != '@openai/codex':
        raise SyncError('registry_latest_is_not_a_stable_codex_release')
    for source_path in ('codex-rs/models-manager/models.json', 'codex-rs/core/models.json'):
        url = f'https://raw.githubusercontent.com/openai/codex/rust-v{version}/{source_path}'
        try:
            raw, _ = fetch(opener, url)
            break
        except SyncError as error:
            if str(error) != 'http_404':
                raise
    else:
        raise SyncError('stable_release_catalog_unavailable')
    catalog = json.loads(raw)
    validate_catalog(catalog)
    if not public_models(catalog):
        raise SyncError('empty_public_model_catalog')
    return version, url, hashlib.sha256(raw).hexdigest(), catalog


def capabilities(row):
    efforts = [value if isinstance(value, str) else value.get('effort')
               for value in row.get('supported_reasoning_levels', [])]
    tiers = [value if isinstance(value, str) else value.get('id')
             for value in row.get('service_tiers', [])]
    fields = {
        'supportsParallelToolCalls': 'supports_parallel_tool_calls',
        'supportsReasoningSummaries': 'supports_reasoning_summaries',
        'supportsVerbosity': 'support_verbosity',
        'supportsImageDetailOriginal': 'supports_image_detail_original',
        'supportsSearchTool': 'supports_search_tool',
        'applyPatchToolType': 'apply_patch_tool_type',
        'webSearchToolType': 'web_search_tool_type',
        'shellType': 'shell_type',
        'defaultReasoningSummary': 'default_reasoning_summary',
    }
    result = {target: row[source] for target, source in fields.items() if source in row}
    result.update(reasoningEfforts=[x for x in efforts if x],
                  serviceTiers=[x for x in tiers if x],
                  inputModalities=row.get('input_modalities', ['text']),
                  supportsTextGeneration=True,
                  endpoints=['responses', 'chat.completions'])
    truncation = row.get('truncation_policy', {})
    if isinstance(truncation, dict):
        result.update(truncationMode=truncation.get('mode'), truncationLimit=truncation.get('limit'))
    return result


PRICE_FIELDS = {
    'input_microusd_per_1m': 'inputMicrousdPer1m',
    'cached_input_microusd_per_1m': 'cachedInputMicrousdPer1m',
    'cache_write_microusd_per_1m': 'cacheWriteMicrousdPer1m',
    'output_microusd_per_1m': 'outputMicrousdPer1m',
}


def managed_model(row, price):
    """Create a complete V2 object. Existing objects are never passed to upsert."""
    rates = {dest: price.get(source) for source, dest in PRICE_FIELDS.items()}
    if any(type(rates[x]) is not int or rates[x] < 0
           for x in ('inputMicrousdPer1m', 'cachedInputMicrousdPer1m', 'outputMicrousdPer1m')):
        raise SyncError('incomplete_official_price')
    tiers = price.get('price_tiers')
    if not tiers or tiers[0].get('min_input_tokens') != 0:
        raise SyncError('incomplete_official_price_tiers')
    previous_threshold = -1
    for tier in tiers:
        threshold = tier.get('min_input_tokens')
        if type(threshold) is not int or threshold <= previous_threshold:
            raise SyncError('invalid_official_price_tier_boundary')
        previous_threshold = threshold
        for source in PRICE_FIELDS:
            rate = tier.get(source)
            if rate is None and source == 'cache_write_microusd_per_1m':
                continue
            if type(rate) is not int or rate < 0:
                raise SyncError('incomplete_official_price_tiers')
    if any(tiers[0].get(source) != price.get(source) for source in PRICE_FIELDS):
        raise SyncError('official_base_price_disagrees_with_first_tier')
    model_tiers = [{'minInputTokens': tier['min_input_tokens'],
                    **{dest: tier.get(source) for source, dest in PRICE_FIELDS.items()}}
                   for tier in tiers]
    return {
        'id': '', 'slug': row['slug'], 'displayName': row['display_name'],
        'description': row.get('description'), 'provider': 'openai',
        'origin': 'custom', 'enabled': True, 'supportedInApi': True,
        'visibility': 'list', 'sortOrder': row.get('priority', 100),
        'contextWindow': row.get('context_window'),
        'maxContextWindow': row.get('max_context_window', row.get('context_window')),
        'defaultReasoningEffort': row.get('default_reasoning_level'),
        'capabilities': capabilities(row), 'instructionsMode': 'passthrough',
        'instructionsText': None, 'fastPolicy': 'passthrough', 'tags': ['codex-daily-sync'],
        'price': {'priceStatus': 'official', 'priceSource': price['price_source'], **rates},
        'priceTiers': model_tiers, 'permissionGroupIds': [],
        'routes': [{'sourceKind': 'account_pool', 'sourceId': 'default',
                    'upstreamModel': row['slug'], 'enabled': True, 'priority': 0, 'weight': 1}],
        'createdAt': 0, 'updatedAt': 0,
    }


class Gateway:
    def __init__(self, config):
        self.config = config
        self.local = make_opener()
        self.token = Path(config['rpc_token_file']).read_text().strip()
        self.saved = json.loads(Path(config['probe_key_file']).read_text())
        if not isinstance(self.saved, dict) or any(not isinstance(self.saved.get(name), str) or not self.saved[name].strip() for name in ('id', 'key')):
            raise SyncError('probe_key_file_invalid')
        if not self.token:
            raise SyncError('rpc_token_file_empty')

    def rpc(self, method, params=None):
        raw, _ = fetch(self.local, self.config['gateway_url'] + '/rpc',
                       data={'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params or {}},
                       headers={'X-CodexManager-Rpc-Token': self.token}, timeout=90)
        value = json.loads(raw)
        result = value.get('result')
        if value.get('error') or (isinstance(result, dict) and result.get('error')):
            raise SyncError('rpc_failed:' + method)
        return result

    def keys(self):
        connection = sqlite3.connect(Path(self.config['database']).as_uri() + '?mode=ro', uri=True)
        try:
            connection.execute('PRAGMA query_only=ON')
            identity = connection.execute('SELECT name,status FROM api_keys WHERE id=?',
                                          (self.saved['id'],)).fetchone()
            if not identity or identity[1] != 'active' or ('probe_key_name' in self.config and identity[0] != self.config['probe_key_name']):
                raise SyncError('probe_key_identity_invalid')
            rows = list(connection.execute(
                "SELECT k.id,s.key_value FROM api_keys k JOIN api_key_secrets s ON s.key_id=k.id "
                "WHERE k.status='active' AND k.upstream_provider='openai' "
                "AND COALESCE(k.rotation_strategy,'account_rotation')='account_rotation'"))
            if not any(key_id == self.saved['id'] and secret == self.saved['key'] for key_id, secret in rows):
                raise SyncError('probe_key_secret_mismatch')
            return rows
        finally:
            connection.close()

    def model_catalog(self, key_id, secret, version, *, require_fresh=True):
        raw, _ = fetch(self.local, self.config['gateway_url'] + '/v1/models',
                       headers={'Authorization': 'Bearer ' + secret}, timeout=90)
        value = json.loads(raw)
        validate_catalog(value)
        if require_fresh:
            path = Path(self.config['official_cache_dir']) / (hashlib.sha256(key_id.encode()).hexdigest() + '.json')
            try:
                cache = json.loads(path.read_text())
                age = time.time() - snapshot_timestamp(cache['fetched_at'])
                if cache['client_version'] != version or age > 600 or age < -60:
                    raise SyncError('official_catalog_not_fresh')
            except (KeyError, ValueError, OSError, TypeError):
                raise SyncError('official_cache_unavailable') from None
        return value

    def response(self, payload):
        start = time.monotonic()
        raw, content_type = fetch(self.local, self.config['gateway_url'] + '/v1/responses',
                                  data=payload,
                                  headers={'Authorization': 'Bearer ' + self.saved['key']}, timeout=100)
        events = []
        try:
            value = json.loads(raw)
        except ValueError:
            try:
                events = [json.loads(line[6:]) for line in raw.decode().splitlines()
                          if line.startswith('data: ') and line[6:].strip() != '[DONE]']
            except (ValueError, UnicodeDecodeError):
                raise SyncError('response_parse_failed') from None
            completed = [event['response'] for event in events if event.get('type') == 'response.completed']
            if not completed:
                raise SyncError('response_stream_incomplete')
            value = completed[-1]
        if not isinstance(value, dict) or value.get('status') != 'completed' or value.get('model') != payload['model']:
            raise SyncError('response_not_completed_with_requested_model')
        if payload.get('stream') and (not content_type.startswith('text/event-stream') or not events or not any(event.get('type') == 'response.created' for event in events)):
            raise SyncError('streaming_contract_failed')
        text = ''.join(part.get('text', '') for item in value.get('output', [])
                       for part in item.get('content', []) if part.get('type') == 'output_text')
        safe = {'requested_model': payload['model'], 'response_model': value.get('model'),
                'http_status': 200, 'status': value['status'], 'stream': payload.get('stream', False),
                'seconds': round(time.monotonic() - start, 3),
                'input_tokens': value.get('usage', {}).get('input_tokens'),
                'output_tokens': value.get('usage', {}).get('output_tokens')}
        return value, text, safe

    def smoke(self, row, *, tools=False):
        efforts = capabilities(row)['reasoningEfforts']
        effort = next((x for x in ('none', 'minimal', 'low') if x in efforts), row.get('default_reasoning_level'))
        base = {'model': row['slug'], 'store': False, 'max_output_tokens': 256}
        if effort:
            base['reasoning'] = {'effort': effort}
        value, output, evidence = self.response({**base, 'input': 'Reply exactly with OK.', 'stream': True})
        if output.strip() != 'OK':
            raise SyncError('canary_output_invalid')
        results = [evidence]
        if tools:
            tool = {'type': 'function', 'name': 'relay_model_check',
                    'description': 'Return the literal OK for a relay connectivity check.',
                    'parameters': {'type': 'object', 'properties': {'value': {'type': 'string', 'enum': ['OK']}},
                                   'required': ['value'], 'additionalProperties': False}, 'strict': True}
            prompt = {'role': 'user', 'content': 'Call relay_model_check with value OK. After receiving its result, reply exactly OK.'}
            called, _, evidence = self.response({**base, 'input': [prompt], 'tools': [tool],
                                                 'tool_choice': {'type': 'function', 'name': tool['name']}, 'stream': True})
            calls = [item for item in called.get('output', []) if item.get('type') == 'function_call']
            if len(calls) != 1 or calls[0].get('name') != tool['name'] or json.loads(calls[0]['arguments']) != {'value': 'OK'}:
                raise SyncError('tool_call_invalid')
            results.append(evidence)
            followup = [prompt, calls[0], {'type': 'function_call_output', 'call_id': calls[0]['call_id'], 'output': 'OK'}]
            _, output, evidence = self.response({**base, 'input': followup, 'tools': [tool], 'stream': True})
            if output.strip() != 'OK':
                raise SyncError('tool_followup_invalid')
            results.append(evidence)
        return results


def backup_caches(config, keys):
    saved = {}
    for key_id, _ in keys:
        name = hashlib.sha256(key_id.encode()).hexdigest() + '.json'
        path = Path(config['official_cache_dir']) / name
        if path.exists():
            with path.open('rb') as source:
                metadata = os.fstat(source.fileno())
                saved[name] = {'content': source.read(), 'mode': stat.S_IMODE(metadata.st_mode),
                               'uid': metadata.st_uid, 'gid': metadata.st_gid}
    return saved


def synchronize(config, state_dir, *, apply=False, smoke_new=False):
    proxy = config.get('outbound_proxy')
    remote = make_opener(proxy)
    version, source_url, source_hash, catalog = read_sources(remote)
    rows = public_models(catalog)
    gateway = Gateway(config)
    keys = gateway.keys()
    settings = gateway.rpc('appSettings/get')
    previous_version = settings['gatewayUserAgentVersion']
    version_changed = previous_version != version
    report = {'checked_at': utc_now(), 'mode': 'apply' if apply else 'dry_run',
              'cli_version': version, 'previous_gateway_client_version': previous_version,
              'source_url': source_url, 'source_sha256': source_hash,
              'cli_public_models': [row['slug'] for row in rows],
              'cli_hidden_models': [row['slug'] for row in catalog['models'] if row['visibility'] != 'list'],
              'added': [], 'pending': [], 'existing_preserved': [], 'smoke_checks': []}
    old_caches = backup_caches(config, keys) if apply and version_changed else {}
    switched = False
    try:
        if version_changed:
            if not apply:
                report['pending'].append({'reason': 'gateway_version_update_required', 'version': version})
                return report
            gateway.rpc('appSettings/set', {'gatewayUserAgentVersion': version})
            switched = True
        upstream = gateway.model_catalog(gateway.saved['id'], gateway.saved['key'], version)
        upstream_ids = {row['slug'] for row in upstream['models'] if row.get('supported_in_api') is True}
        managed_result = gateway.rpc('apikey/managedModelListV2', {'includeHidden': True})
        managed_rows = managed_result['items']
        existing = {row['slug']: row for row in managed_rows}
        candidates = []
        for row in rows:
            slug = row['slug']
            if slug in existing:
                report['existing_preserved'].append(slug)
            elif slug not in upstream_ids:
                report['pending'].append({'model': slug, 'reason': 'not_authorized_by_official_upstream'})
            else:
                candidates.append(row)
        prices = {}
        if candidates:
            from official_prices import parse_official_prices, pricing_component_url
            try:
                price_raw, _ = fetch(remote, PRICING)
                price_html = price_raw.decode()
                component_raw, _ = fetch(remote, pricing_component_url(price_html))
                prices = parse_official_prices(price_html, component_js=component_raw.decode())
            except Exception as error:
                report['pricing_error'] = type(error).__name__
        approved = []
        for row in candidates:
            price = prices.get(row['slug'])
            if not price:
                report['pending'].append({'model': row['slug'], 'reason': 'official_price_unconfirmed'})
                continue
            if not apply and not smoke_new:
                report['pending'].append({'model': row['slug'], 'reason': 'awaiting_live_smoke_check'})
                continue
            try:
                model = managed_model(row, price)
                checks = gateway.smoke(row, tools=True)
                report['smoke_checks'].append({'model': row['slug'], 'tests': checks})
                approved.append(model)
            except Exception as error:
                report['pending'].append({'model': row['slug'], 'reason': 'live_verification_failed',
                                          'error': str(error) if isinstance(error, SyncError) else type(error).__name__})
        if apply:
            # Refresh through real authenticated endpoints; never substitute a bundled catalog for account authorization.
            def refresh_key(pair):
                key_id, secret = pair
                gateway.model_catalog(key_id, secret, version)
                return {'key_id': key_id, 'http_status': 200}
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                report['key_catalog_checks'] = list(pool.map(refresh_key, keys))
            ready = {slug for slug, item in existing.items() if item['enabled'] and item['supportedInApi'] and item['visibility'] == 'list'}
            ready.update(item['slug'] for item in approved)
            canary_options = [row for row in rows if row['slug'] in upstream_ids and row['slug'] in ready]
            canary = next((row for row in canary_options if row['slug'] == config.get('canary_model')),
                          min(canary_options, key=lambda row: row.get('priority', 100)) if canary_options else None)
            if not canary:
                raise SyncError('no_usable_canary_model')
            report['canary'] = gateway.smoke(canary)
            if approved:
                # Recheck after slow probes so a model added manually during verification is preserved.
                current = {item['slug'].lower() for item in gateway.rpc(
                    'apikey/managedModelListV2', {'includeHidden': True})['items']}
                added_during_checks = [model['slug'] for model in approved if model['slug'].lower() in current]
                report['concurrent_existing_preserved'] = added_during_checks
                approved = [model for model in approved if model['slug'].lower() not in current]
            if approved:
                params = {'jsonContent': json.dumps({'models': approved}), 'conflictStrategy': 'keep_existing'}
                preview = gateway.rpc('apikey/managedModelImportPreviewV2', params)
                if preview.get('errors'):
                    raise SyncError('model_import_validation_failed')
                result = gateway.rpc('apikey/managedModelImportCommitV2', params)
                report['added'] = result['added']
                report['concurrent_existing_preserved'].extend(result.get('conflicts', []))
                for slug in report['added']:
                    saved = gateway.rpc('apikey/managedModelGetV2', {'slug': slug})
                    if not saved['enabled'] or not saved['supportedInApi'] or saved['price']['priceStatus'] != 'official' or not any(
                        route['enabled'] and route['sourceKind'] == 'account_pool' and route['upstreamModel'] == slug for route in saved['routes']):
                        raise SyncError('post_import_model_verification_failed')
            # This local catalog is suitable for model_catalog_json; it contains no credentials.
            verified_managed = gateway.rpc('apikey/managedModelListV2', {'includeHidden': True})['items']
            enabled = {row['slug'] for row in verified_managed if row['enabled'] and row['supportedInApi'] and row['visibility'] == 'list'}
            export = {'models': [row for row in rows if row['slug'] in enabled and row['slug'] in upstream_ids]}
            report['exported_models'] = [row['slug'] for row in export['models']]
            report['status'] = 'ok' if not report['pending'] else 'pending'
            publish_snapshot(state_dir, catalog, export, report)
        else:
            report['would_add_after_verification'] = [model['slug'] for model in approved]
        return report
    except Exception:
        if switched:
            # Saved snapshots are genuine upstream responses, restored only when rolling back the matching version.
            gateway.rpc('appSettings/set', {'gatewayUserAgentVersion': previous_version})
            for filename, saved in old_caches.items():
                atomic_write(Path(config['official_cache_dir']) / filename, saved['content'], metadata=saved)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True, help='operator JSON; relative paths are resolved beside this file')
    parser.add_argument('--apply', action='store_true', help='verify and add new models through administrator RPC')
    parser.add_argument('--smoke-new', action='store_true', help='test candidates during a dry run without writing managed models')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        config = load_config(args.config)
        state_dir = Path(config['state_dir'])
        state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    except (SyncError, OSError) as error:
        print(json.dumps({'status': 'failed', 'checked_at': utc_now(), 'error': str(error) if isinstance(error, SyncError) else type(error).__name__}))
        return 1
    with (state_dir / 'sync.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(json.dumps({'status': 'already_running'}))
            return 0
        try:
            report = synchronize(config, state_dir, apply=args.apply, smoke_new=args.smoke_new)
            report['status'] = 'ok' if not report['pending'] else 'pending'
            write_json(state_dir / 'last-run.json', report)
            # Log only operational metadata, never raw HTTP/RPC bodies, keys or instructions.
            print(json.dumps({k: report.get(k) for k in ('status', 'checked_at', 'mode', 'cli_version', 'added', 'pending', 'existing_preserved')}, ensure_ascii=False))
            return 0
        except Exception as error:
            report = {'status': 'failed', 'checked_at': utc_now(),
                      'error': str(error) if isinstance(error, SyncError) else type(error).__name__}
            write_json(state_dir / 'last-run.json', report)
            print(json.dumps(report))
            return 1


if __name__ == '__main__':
    sys.exit(main())
