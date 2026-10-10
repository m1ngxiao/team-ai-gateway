import json
import hashlib
import os
import stat
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

import model_sync as sync


def row(slug='gpt-future-sol', **overrides):
    return {'slug': slug, 'display_name': 'Future Sol', 'visibility': 'list',
            'supported_in_api': True, 'priority': 1, 'context_window': 272000,
            'max_context_window': 872000, 'default_reasoning_level': 'low',
            'supported_reasoning_levels': [{'effort': 'low'}], **overrides}


def price():
    rates = {'input_microusd_per_1m': 2000000, 'cached_input_microusd_per_1m': 100000,
             'cache_write_microusd_per_1m': 2500000, 'output_microusd_per_1m': 10000000}
    return {**rates, 'price_source': sync.PRICING,
            'price_tiers': [{'min_input_tokens': 0, **rates},
                            {'min_input_tokens': 272001, **{key: value * 2 for key, value in rates.items()}}]}


class FakeGateway:
    initial = {}
    version = '0.162.1'
    authorized = None
    failure = None
    instances = []

    def __init__(self, config):
        self.saved = {'id': 'check-key', 'key': 'not-a-real-secret'}
        self.models = dict(self.initial)
        self.calls = []
        self.__class__.instances.append(self)

    def keys(self):
        return [('check-key', 'not-a-real-secret')]

    def rpc(self, method, params=None):
        self.calls.append((method, params))
        if method == 'appSettings/get':
            return {'gatewayUserAgentVersion': self.version}
        if method == 'appSettings/set':
            self.version = params['gatewayUserAgentVersion']
            return {}
        if method == 'apikey/managedModelListV2':
            return {'items': list(self.models.values())}
        if method in ('apikey/managedModelImportPreviewV2', 'apikey/managedModelImportCommitV2'):
            models = json.loads(params['jsonContent'])['models']
            self.assert_keep_existing = params['conflictStrategy'] == 'keep_existing'
            added = [model['slug'] for model in models if model['slug'] not in self.models]
            if method.endswith('CommitV2'):
                for model in models:
                    self.models.setdefault(model['slug'], model)
            return {'added': added, 'errors': [], 'conflicts': []}
        if method == 'apikey/managedModelGetV2':
            return self.models[params['slug']]
        raise AssertionError(method)

    def model_catalog(self, key_id, secret, version):
        if self.failure == 'catalog':
            raise sync.SyncError('network_request_failed')
        return {'models': self.authorized}

    def smoke(self, model, **kwargs):
        self.calls.append(('smoke', model['slug']))
        if self.failure == model['slug']:
            raise sync.SyncError('response_stream_incomplete')
        return [{'response_model': model['slug'], 'status': 'completed'}]


class SyncTest(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.root = Path(self.folder.name)
        self.config = {'outbound_proxy': None,
                       'official_cache_dir': str(self.root / 'official'), 'canary_model': 'gpt-canary'}
        FakeGateway.initial = {'gpt-canary': {'slug': 'gpt-canary', 'enabled': True,
                                             'supportedInApi': True, 'visibility': 'list',
                                             'userEdited': True, 'price': {'priceStatus': 'custom'}}}
        FakeGateway.version = '0.162.1'
        FakeGateway.authorized = [row('gpt-canary'), row()]
        FakeGateway.failure = None
        FakeGateway.instances = []
        self.catalog = {'models': [row('gpt-canary'), row(), row('hidden', visibility='hide')]}
        self.prices = {'gpt-future-sol': price()}

    def tearDown(self):
        self.folder.cleanup()

    def run_sync(self, apply=True):
        module = types.SimpleNamespace(parse_official_prices=lambda *a, **k: self.prices,
                                       pricing_component_url=lambda html: 'https://developers.openai.com/_astro/pricing.js')
        with patch.object(sync, 'read_sources', return_value=('0.162.1', 'official-source', 'hash', self.catalog)), \
             patch.object(sync, 'Gateway', FakeGateway), \
             patch.object(sync, 'fetch', return_value=(b'official-fixture', 'text/html')), \
             patch.dict('sys.modules', {'official_prices': module}):
            return sync.synchronize(self.config, self.root / 'state', apply=apply)

    def test_official_hidden_entries_excluded(self):
        self.assertEqual([x['slug'] for x in sync.public_models(self.catalog)], ['gpt-canary', 'gpt-future-sol'])

    def test_empty_and_duplicate_catalog_rejected(self):
        for catalog in ({'models': []}, {'models': [row(), row()]}, {'models': [row('Bearer-secret')]}):
            with self.assertRaises(sync.SyncError):
                sync.public_models(catalog)

    def test_add_only_after_real_checks_and_keep_existing(self):
        before = json.loads(json.dumps(FakeGateway.initial['gpt-canary']))
        result = self.run_sync()
        gateway = FakeGateway.instances[-1]
        self.assertEqual(result['added'], ['gpt-future-sol'])
        self.assertEqual(gateway.models['gpt-canary'], before)
        self.assertTrue(gateway.assert_keep_existing)
        methods = [x[0] for x in gateway.calls]
        self.assertLess(methods.index('smoke'), methods.index('apikey/managedModelImportCommitV2'))
        added = gateway.models['gpt-future-sol']
        self.assertEqual(added['routes'][0]['upstreamModel'], 'gpt-future-sol')
        self.assertEqual(added['priceTiers'][1]['minInputTokens'], 272001)
        self.assertEqual(result['exported_models'], ['gpt-canary', 'gpt-future-sol'])

    def test_second_run_is_additive_and_preserves_imported_model(self):
        self.run_sync()
        FakeGateway.initial = json.loads(json.dumps(FakeGateway.instances[-1].models))
        before = json.loads(json.dumps(FakeGateway.initial))
        result = self.run_sync()
        self.assertEqual(result['added'], [])
        self.assertEqual(FakeGateway.instances[-1].models, before)
        self.assertFalse(any('Commit' in method for method, _ in FakeGateway.instances[-1].calls))

    def test_existing_disabled_route_and_custom_price_are_untouched(self):
        existing = sync.managed_model(row(), price())
        existing.update(enabled=False, visibility='hide', userEdited=True)
        existing['price'].update(priceStatus='custom', inputMicrousdPer1m=123)
        existing['routes'][0].update(enabled=False, upstreamModel='operator-alias')
        FakeGateway.initial['gpt-future-sol'] = existing
        before = json.loads(json.dumps(existing))
        result = self.run_sync()
        self.assertEqual(result['added'], [])
        self.assertEqual(FakeGateway.instances[-1].models['gpt-future-sol'], before)
        self.assertEqual(result['exported_models'], ['gpt-canary'])

    def test_addition_during_slow_probe_is_preserved(self):
        original_rpc = FakeGateway.rpc
        existing = sync.managed_model(row(), price())
        existing['price'].update(priceStatus='custom', inputMicrousdPer1m=456)
        def concurrent_rpc(gateway, method, params=None):
            if method == 'apikey/managedModelListV2' and any(call[0] == 'smoke' for call in gateway.calls):
                gateway.models.setdefault('gpt-future-sol', json.loads(json.dumps(existing)))
            return original_rpc(gateway, method, params)
        with patch.object(FakeGateway, 'rpc', concurrent_rpc):
            result = self.run_sync()
        self.assertEqual(result['added'], [])
        self.assertEqual(result['concurrent_existing_preserved'], ['gpt-future-sol'])
        self.assertEqual(FakeGateway.instances[-1].models['gpt-future-sol'], existing)

    def test_import_conflicts_reported_without_overwriting(self):
        original_rpc = FakeGateway.rpc
        existing = sync.managed_model(row(), price())
        existing['price'].update(priceStatus='custom', inputMicrousdPer1m=789)
        def conflicting_rpc(gateway, method, params=None):
            if method == 'apikey/managedModelImportCommitV2':
                gateway.models['gpt-future-sol'] = json.loads(json.dumps(existing))
                gateway.calls.append((method, params))
                self.assertEqual(params['conflictStrategy'], 'keep_existing')
                return {'added': [], 'conflicts': ['gpt-future-sol']}
            return original_rpc(gateway, method, params)
        with patch.object(FakeGateway, 'rpc', conflicting_rpc):
            result = self.run_sync()
        self.assertEqual(result['added'], [])
        self.assertEqual(result['concurrent_existing_preserved'], ['gpt-future-sol'])
        self.assertEqual(FakeGateway.instances[-1].models['gpt-future-sol'], existing)

    def test_missing_price_stays_pending_without_writing(self):
        self.prices = {}
        result = self.run_sync()
        self.assertEqual(result['added'], [])
        self.assertEqual(result['pending'][0]['reason'], 'official_price_unconfirmed')
        self.assertNotIn('gpt-future-sol', FakeGateway.instances[-1].models)

    def test_failed_streaming_stays_pending(self):
        FakeGateway.failure = 'gpt-future-sol'
        result = self.run_sync()
        self.assertEqual(result['pending'][0]['reason'], 'live_verification_failed')
        self.assertEqual(result['added'], [])

    def test_official_authorization_is_required(self):
        FakeGateway.authorized = [row('gpt-canary')]
        result = self.run_sync()
        self.assertEqual(result['pending'][0]['reason'], 'not_authorized_by_official_upstream')
        self.assertNotIn('gpt-future-sol', FakeGateway.instances[-1].models)

    def test_version_failure_rolls_back_before_any_import(self):
        FakeGateway.version = '0.161.0'
        FakeGateway.failure = 'catalog'
        with self.assertRaises(sync.SyncError):
            self.run_sync()
        gateway = FakeGateway.instances[-1]
        self.assertEqual(gateway.version, '0.161.0')
        self.assertEqual([x[1]['gatewayUserAgentVersion'] for x in gateway.calls if x[0] == 'appSettings/set'], ['0.162.1', '0.161.0'])
        self.assertFalse(any('Commit' in x[0] for x in gateway.calls))

    def test_cache_rollback_preserves_content_mode_and_owner_before_replace(self):
        directory = Path(self.config['official_cache_dir'])
        directory.mkdir()
        path = directory / (hashlib.sha256(b'check-key').hexdigest() + '.json')
        path.write_bytes(b'previous genuine cache fixture')
        path.chmod(0o640)
        original = path.stat()
        FakeGateway.version = '0.161.0'
        replace = sync.os.replace

        def failed_catalog(*args):
            path.write_bytes(b'refreshed cache fixture')
            path.chmod(0o600)
            raise sync.SyncError('network_request_failed')

        def replace_after_metadata(temporary, destination):
            if Path(destination) == path:
                restored = Path(temporary).stat()
                self.assertEqual((stat.S_IMODE(restored.st_mode), restored.st_uid, restored.st_gid),
                                 (0o640, original.st_uid, original.st_gid))
            return replace(temporary, destination)

        with patch.object(FakeGateway, 'model_catalog', side_effect=failed_catalog), \
             patch.object(sync.os, 'replace', side_effect=replace_after_metadata), self.assertRaises(sync.SyncError):
            self.run_sync()
        restored = path.stat()
        self.assertEqual(path.read_bytes(), b'previous genuine cache fixture')
        self.assertEqual((stat.S_IMODE(restored.st_mode), restored.st_uid, restored.st_gid),
                         (0o640, original.st_uid, original.st_gid))
        self.assertEqual(FakeGateway.instances[-1].version, '0.161.0')

    def test_foreign_cache_owner_is_restored_before_atomic_replace(self):
        directory = Path(self.config['official_cache_dir'])
        directory.mkdir()
        path = directory / (hashlib.sha256(b'check-key').hexdigest() + '.json')
        path.write_bytes(b'previous cache fixture')
        # Simulate a gateway-owned cache without changing system ownership.
        current = path.stat()
        foreign_uid, foreign_gid = current.st_uid + 10001, current.st_gid + 10001
        metadata = types.SimpleNamespace(st_uid=foreign_uid, st_gid=foreign_gid, st_mode=0o100640)
        with patch.object(sync.os, 'fstat', return_value=metadata):
            saved = next(iter(sync.backup_caches(self.config, [('check-key', 'synthetic')]).values()))
        self.assertEqual((saved['mode'], saved['uid'], saved['gid']), (0o640, foreign_uid, foreign_gid))
        events = []
        replace = sync.os.replace

        def restore_owner(fd, uid, gid):
            events.append(('owner', uid, gid))

        def publish(temporary, destination):
            self.assertEqual(events, [('owner', foreign_uid, foreign_gid)])
            self.assertEqual(stat.S_IMODE(Path(temporary).stat().st_mode), 0o640)
            events.append(('replace',))
            return replace(temporary, destination)

        with patch.object(sync.os, 'fchown', side_effect=restore_owner), patch.object(sync.os, 'replace', side_effect=publish):
            sync.atomic_write(path, saved['content'], metadata=saved)
        self.assertEqual(events[-1], ('replace',))

    def test_cache_metadata_failure_preserves_existing_file(self):
        path = self.root / 'cache.json'
        path.write_bytes(b'current gateway cache fixture')
        current = path.stat()
        metadata = {'uid': current.st_uid + 10001, 'gid': current.st_gid + 10001, 'mode': 0o640}
        with patch.object(sync.os, 'fchown', side_effect=PermissionError), self.assertRaises(PermissionError):
            sync.atomic_write(path, b'rollback cache fixture', metadata=metadata)
        self.assertEqual(path.read_bytes(), b'current gateway cache fixture')
        self.assertFalse(list(self.root.glob('.cache.json-*')))

    def test_dry_run_writes_no_models_and_performs_no_smoke(self):
        result = self.run_sync(apply=False)
        self.assertEqual(result['added'], [])
        self.assertFalse(any(x[0] == 'smoke' or 'Commit' in x[0] for x in FakeGateway.instances[-1].calls))
        self.assertFalse((self.root / 'state' / 'last-success.json').exists())

    def test_atomic_files_private(self):
        path = self.root / 'state' / 'report.json'
        sync.write_json(path, {'ok': True})
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_real_cache_rfc3339_timestamp_and_numeric_compatibility(self):
        self.assertEqual(sync.snapshot_timestamp('2026-10-10T00:00:00Z'), 1791590400)
        self.assertEqual(sync.snapshot_timestamp(1791590400), 1791590400)

    def test_retired_canary_falls_back_to_current_verified_model(self):
        self.catalog['models'] = [row()]
        result = self.run_sync()
        self.assertEqual(result['added'], ['gpt-future-sol'])
        self.assertEqual(result['canary'][0]['response_model'], 'gpt-future-sol')

    def test_empty_export_never_replaces_previous_catalog(self):
        state = self.root / 'state'
        state.mkdir()
        sync.publish_snapshot(state, self.catalog, self.catalog, {'cli_version': '0.162.0'})
        old_pointer = (state / 'current').readlink()
        with self.assertRaises(sync.SyncError):
            sync.publish_snapshot(state, self.catalog, {'models': []}, {'cli_version': '0.162.1'})
        self.assertEqual((state / 'current').readlink(), old_pointer)

    def test_bundle_write_failure_preserves_all_previous_documents(self):
        state = self.root / 'state'
        state.mkdir()
        old = {'models': [row('old-model')]}
        sync.publish_snapshot(state, old, old, {'cli_version': '0.161.0'})
        write = sync.write_json
        def fail_source(path, value):
            if path.name == 'source-catalog.json':
                raise OSError('fixture write failure')
            write(path, value)
        with patch.object(sync, 'write_json', side_effect=fail_source), self.assertRaises(OSError):
            sync.publish_snapshot(state, self.catalog, self.catalog, {'cli_version': '0.162.1'})
        self.assertEqual(json.loads((state / 'gateway-models.json').read_text()), old)
        self.assertEqual(json.loads((state / 'source-catalog.json').read_text()), old)
        self.assertEqual(json.loads((state / 'last-success.json').read_text())['cli_version'], '0.161.0')


if __name__ == '__main__':
    unittest.main()
