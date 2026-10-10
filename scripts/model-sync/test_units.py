import json
from pathlib import Path
import tempfile
import unittest

import model_sync as sync
import render_units


class UnitTemplateTest(unittest.TestCase):
    def test_rendered_paths_schedule_and_write_scope(self):
        with tempfile.TemporaryDirectory(prefix='units with spaces ') as folder:
            root = Path(folder)
            config = root / 'config.json'
            config.write_text(json.dumps({'gateway_url': 'http://127.0.0.1:12345', 'database': 'data/gateway.db',
                'rpc_token_file': 'data/rpc-token', 'probe_key_file': 'probe-key.json', 'official_cache_dir': 'data/official-model-catalogs', 'state_dir': 'state'}))
            files = render_units.render(config, root / 'model_sync.py', '/usr/bin/python3', 'root', '*-*-* 03:00:00 UTC')
            service = files['team-ai-gateway-model-sync.service']
            timer = files['team-ai-gateway-model-sync.timer']
            self.assertIn('--config "' + str(config) + '" --apply', service)
            self.assertIn('ReadWritePaths="' + str(root / 'state') + '" "' + str(root / 'data/official-model-catalogs') + '"', service)
            self.assertIn('ProtectSystem=strict', service)
            self.assertIn('OnCalendar=*-*-* 03:00:00 UTC', timer)
            self.assertIn('Persistent=true', timer)
            self.assertNotIn('@CONFIG@', service)

    def test_control_characters_cannot_inject_unit_directives(self):
        with self.assertRaises(sync.SyncError):
            render_units.single_line('daily\nExecStart=unwanted')
        with self.assertRaises(sync.SyncError):
            render_units.quote('path\nnext-line')

    def test_systemd_argument_escaping(self):
        self.assertEqual(render_units.quote('path with %literal $text', executable_argument=True), '"path with %%literal $$text"')


if __name__ == '__main__':
    unittest.main()
