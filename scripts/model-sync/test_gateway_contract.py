"""Synthetic protocol tests; no server, registry, accounts, or network required."""
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import urllib.request

import model_sync as sync
from test_model_sync import row, price


def completed(slug, output):
    return {'id': 'synthetic-response', 'model': slug, 'status': 'completed',
            'output': output, 'usage': {'input_tokens': 1, 'output_tokens': 1}}


def stream(value, *, created=True, complete=True):
    events = [{'type': 'response.created', 'response': {'status': 'in_progress'}}] if created else []
    if complete:
        events.append({'type': 'response.completed', 'response': value})
    return ''.join('data: ' + json.dumps(event) + '\n\n' for event in events).encode(), 'text/event-stream; charset=utf-8'


def message(text='OK'):
    return {'type': 'message', 'content': [{'type': 'output_text', 'text': text}]}


class GatewayContractTest(unittest.TestCase):
    def gateway(self):
        gateway = object.__new__(sync.Gateway)
        gateway.config = {'gateway_url': 'http://127.0.0.1:12345'}
        gateway.local = object()
        gateway.saved = {'id': 'synthetic-key', 'key': 'synthetic-secret'}
        return gateway

    def test_streamed_function_call_and_result_round_trip(self):
        model = row()
        call = {'id': 'synthetic-function-item', 'type': 'function_call', 'call_id': 'synthetic-call',
                'name': 'relay_model_check', 'arguments': '{"value":"OK"}'}
        responses = [stream(completed(model['slug'], [message()])),
                     stream(completed(model['slug'], [call])),
                     stream(completed(model['slug'], [message()]))]
        with patch.object(sync, 'fetch', side_effect=responses) as fetch:
            checks = self.gateway().smoke(model, tools=True)
        self.assertEqual(len(checks), 3)
        payloads = [item.kwargs['data'] for item in fetch.call_args_list]
        self.assertTrue(all(payload['stream'] for payload in payloads))
        self.assertEqual(payloads[1]['tool_choice'], {'type': 'function', 'name': 'relay_model_check'})
        self.assertEqual(payloads[2]['input'][-1], {'type': 'function_call_output', 'call_id': 'synthetic-call', 'output': 'OK'})
        self.assertTrue(all(check['response_model'] == model['slug'] for check in checks))

    def test_stream_requires_sse_type_created_and_completed(self):
        value = completed('gpt-future-sol', [message()])
        failures = [stream(value, created=False), stream(value, complete=False),
                    (json.dumps(value).encode(), 'application/json'),
                    (stream(value)[0], 'application/json')]
        for response in failures:
            with patch.object(sync, 'fetch', return_value=response), self.assertRaises(sync.SyncError):
                self.gateway().response({'model': 'gpt-future-sol', 'stream': True})

    def test_model_alias_or_failed_response_is_rejected(self):
        for value in [completed('different-model', [message()]), {'model': 'gpt-future-sol', 'status': 'failed'}]:
            with patch.object(sync, 'fetch', return_value=stream(value)), self.assertRaises(sync.SyncError):
                self.gateway().response({'model': 'gpt-future-sol', 'stream': True})

    def test_wrong_tool_arguments_or_followup_output_is_rejected(self):
        model = row()
        call = {'type': 'function_call', 'call_id': 'synthetic-call', 'name': 'relay_model_check', 'arguments': '{"value":"unexpected"}'}
        responses = [stream(completed(model['slug'], [message()])), stream(completed(model['slug'], [call]))]
        with patch.object(sync, 'fetch', side_effect=responses), self.assertRaisesRegex(sync.SyncError, 'tool_call_invalid'):
            self.gateway().smoke(model, tools=True)
        call['arguments'] = '{"value":"OK"}'
        responses = [stream(completed(model['slug'], [message()])), stream(completed(model['slug'], [call])), stream(completed(model['slug'], [message('wrong')]))]
        with patch.object(sync, 'fetch', side_effect=responses), self.assertRaisesRegex(sync.SyncError, 'tool_followup_invalid'):
            self.gateway().smoke(model, tools=True)

    def test_probe_identity_and_secret_against_synthetic_readonly_database(self):
        with tempfile.TemporaryDirectory() as folder:
            db_path = Path(folder) / 'gateway.db'
            with sqlite3.connect(db_path) as db:
                db.executescript('CREATE TABLE api_keys(id TEXT, name TEXT, status TEXT, upstream_provider TEXT, rotation_strategy TEXT); CREATE TABLE api_key_secrets(key_id TEXT, key_value TEXT);')
                db.execute('INSERT INTO api_keys VALUES(?,?,?,?,?)', ('synthetic-key', 'Example operator', 'active', 'openai', 'account_rotation'))
                db.execute('INSERT INTO api_key_secrets VALUES(?,?)', ('synthetic-key', 'synthetic-secret'))
            gateway = self.gateway()
            gateway.config['database'] = str(db_path)
            self.assertEqual(gateway.keys(), [('synthetic-key', 'synthetic-secret')])
            gateway.saved['key'] = 'mismatch'
            with self.assertRaisesRegex(sync.SyncError, 'probe_key_secret_mismatch'):
                gateway.keys()


class ConfigAndSourceTest(unittest.TestCase):
    def config(self, root):
        return {'gateway_url': 'http://127.0.0.1:12345/', 'database': 'gateway/gateway.db',
                'rpc_token_file': 'gateway/rpc-token', 'probe_key_file': 'probe-key.json',
                'official_cache_dir': 'gateway/official-model-catalogs', 'state_dir': 'sync-state', 'outbound_proxy': None}

    def test_relative_paths_follow_config_directory(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / 'config.json'
            path.write_text(json.dumps(self.config(root)))
            config = sync.load_config(path)
            self.assertEqual(config['database'], str(root / 'gateway/gateway.db'))
            self.assertEqual(config['state_dir'], str(root / 'sync-state'))
            self.assertEqual(config['gateway_url'], 'http://127.0.0.1:12345')

    def test_invalid_config_is_safe_and_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'config.json'
            for updates in [{'gateway_url': 'http://remote.example'}, {'gateway_url': 'https://user:secret@example.invalid'}, {'state_dir': ''}, {'unknown': 'synthetic-secret'}, {'outbound_proxy': 'socks5://localhost:1234'}]:
                config = self.config(Path(folder))
                config.update(updates)
                path.write_text(json.dumps(config))
                with self.assertRaises(sync.SyncError) as failure:
                    sync.load_config(path)
                self.assertNotIn('secret', str(failure.exception))

    def test_registry_stable_release_and_matching_official_tag(self):
        catalog = {'models': [row()]}
        replies = [(json.dumps({'name': '@openai/codex', 'version': '1.2.3'}).encode(), 'application/json'), (json.dumps(catalog).encode(), 'application/json')]
        with patch.object(sync, 'fetch', side_effect=replies) as fetch:
            version, url, digest, result = sync.read_sources(object())
        self.assertEqual(version, '1.2.3')
        self.assertEqual(url, 'https://raw.githubusercontent.com/openai/codex/rust-v1.2.3/codex-rs/models-manager/models.json')
        self.assertEqual(fetch.call_args_list[0].args[1], sync.REGISTRY)
        self.assertEqual(len(digest), 64)
        self.assertEqual(result, catalog)

    def test_prerelease_wrong_package_and_missing_version_are_rejected(self):
        for release in [{'name': '@openai/codex', 'version': '1.2.3-beta.1'}, {'name': 'another-package', 'version': '1.2.3'}, {'name': '@openai/codex', 'version': None}]:
            with patch.object(sync, 'fetch', return_value=(json.dumps(release).encode(), 'application/json')), self.assertRaises(sync.SyncError):
                sync.read_sources(object())

    def test_legacy_official_catalog_path_is_only_used_on_404(self):
        release = (json.dumps({'name': '@openai/codex', 'version': '1.2.3'}).encode(), 'application/json')
        catalog = (json.dumps({'models': [row()]}).encode(), 'application/json')
        with patch.object(sync, 'fetch', side_effect=[release, sync.SyncError('http_404'), catalog]):
            _, url, _, _ = sync.read_sources(object())
        self.assertTrue(url.endswith('/codex-rs/core/models.json'))
        with patch.object(sync, 'fetch', side_effect=[release, sync.SyncError('http_503')]), self.assertRaisesRegex(sync.SyncError, 'http_503'):
            sync.read_sources(object())

    def test_cross_origin_redirect_rejects_credential_forwarding(self):
        request = urllib.request.Request('https://gateway.example/v1/models', headers={'Authorization': 'Bearer synthetic-secret'})
        with self.assertRaisesRegex(sync.SyncError, 'cross_origin_redirect_rejected'):
            sync.SameOriginRedirect().redirect_request(request, None, 302, 'moved', {}, 'https://another.example/models')

    def test_price_tiers_cannot_lose_or_misorder_rates(self):
        for mutate in [lambda p: p['price_tiers'][1].update(min_input_tokens=0), lambda p: p['price_tiers'][1].pop('output_microusd_per_1m'), lambda p: p['price_tiers'][0].update(input_microusd_per_1m=1)]:
            rates = price()
            mutate(rates)
            with self.assertRaises(sync.SyncError):
                sync.managed_model(row(), rates)


if __name__ == '__main__':
    unittest.main()
