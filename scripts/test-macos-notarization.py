#!/usr/bin/env python3
"""Exercise Apple workflow failures without certificates, credentials or Apple network calls."""
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
MOCK = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['MOCK_EVENTS'], 'a') as output:
    output.write(json.dumps([name, *args]) + '\n')
if name == 'xcrun' and args[:2] == ['notarytool', 'submit']:
    portable = args[2].endswith('.zip')
    status = os.environ.get('MOCK_PORTABLE_STATUS' if portable else 'MOCK_DMG_STATUS', 'Accepted')
    print(json.dumps({'id': '12345678-1234-1234-1234-123456789012', 'status': status}))
    sys.exit(int(os.environ.get('MOCK_SUBMIT_EXIT', '0')))
if name == 'xcrun' and args[:2] == ['notarytool', 'store-credentials']:
    sys.exit(int(os.environ.get('MOCK_STORE_EXIT', '0')))
if name == 'ditto':
    pathlib.Path(args[-1]).touch()
if name == 'security':
    if args[0] == 'list-keychains' and '-s' not in args:
        print('"/original/login.keychain-db"')
    if args[0] == 'create-keychain':
        pathlib.Path(args[-1]).touch()
    if args[0] == 'find-identity':
        print('1) ' + 'A' * 40 + ' "Developer ID Application: Test (MOCKTEAM01)"')
    if args[0] == 'delete-keychain':
        pathlib.Path(args[-1]).unlink()
'''


class NotarizationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='codeyard-notary-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        tools = self.root / 'tools'
        tools.mkdir()
        for name in ['xcrun', 'codesign', 'spctl', 'ditto', 'security']:
            path = tools / name
            path.write_text(MOCK)
            path.chmod(0o755)
        for folder in ['portable', 'Codeyard.app', 'runner-temp']:
            (self.root / folder).mkdir()
        (self.root / 'Codeyard.dmg').touch()
        self.events = self.root / 'events.jsonl'
        self.env = dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}',
                        MOCK_EVENTS=str(self.events), NOTARY_PROFILE='mock-profile',
                        RUNNER_TEMP=str(self.root / 'runner-temp'), GITHUB_ENV=str(self.root / 'github-env'))

    def calls(self):
        return [json.loads(line) for line in self.events.read_text().splitlines()]

    def notarize(self, **overrides):
        return subprocess.run(['bash', str(SCRIPTS / 'notarize-macos.sh'),
                               'Codeyard.dmg', 'portable', 'Codeyard.app'],
                              cwd=self.root, env=dict(self.env, **overrides), capture_output=True, text=True)

    def test_both_archives_accepted_before_stapling_and_gatekeeper(self):
        result = self.notarize()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        submissions = [i for i, call in enumerate(calls) if call[:3] == ['xcrun', 'notarytool', 'submit']]
        staple = next(i for i, call in enumerate(calls) if call[:3] == ['xcrun', 'stapler', 'staple'])
        self.assertEqual(len(submissions), 2)
        self.assertGreater(staple, max(submissions))
        self.assertEqual(sum(call[0] == 'spctl' for call in calls), 2)

    def test_invalid_dmg_with_zero_exit_code_stops_before_portable_and_staple(self):
        result = self.notarize(MOCK_DMG_STATUS='Invalid')
        self.assertNotEqual(result.returncode, 0)
        calls = self.calls()
        self.assertEqual(sum(call[:3] == ['xcrun', 'notarytool', 'submit'] for call in calls), 1)
        self.assertFalse(any(call[:2] == ['xcrun', 'stapler'] for call in calls))

    def test_invalid_portable_stops_before_staple(self):
        self.assertNotEqual(self.notarize(MOCK_PORTABLE_STATUS='Invalid').returncode, 0)
        self.assertFalse(any(call[:2] == ['xcrun', 'stapler'] for call in self.calls()))

    def test_submit_failure_cannot_be_treated_as_success(self):
        self.assertNotEqual(self.notarize(MOCK_SUBMIT_EXIT='69').returncode, 0)
        self.assertFalse(any(call[:2] == ['xcrun', 'stapler'] for call in self.calls()))

    def test_failed_credential_setup_still_removes_p12_and_can_be_cleaned(self):
        env = dict(self.env, MACOS_CERTIFICATE_P12=base64.b64encode(b'fake-p12').decode(),
                   MACOS_CERTIFICATE_PASSWORD='fake-password', APPLE_ID='test@example.invalid',
                   APPLE_APP_SPECIFIC_PASSWORD='fake-password', APPLE_TEAM_ID='MOCKTEAM01', MOCK_STORE_EXIT='1')
        setup = subprocess.run(['bash', str(SCRIPTS / 'macos-signing-keychain.sh'), 'setup'],
                               env=env, cwd=self.root, capture_output=True, text=True)
        self.assertNotEqual(setup.returncode, 0)
        work = self.root / 'runner-temp/codeyard-apple-signing'
        self.assertFalse((work / 'certificate.p12').exists())
        self.assertFalse((self.root / 'github-env').exists())
        subprocess.run(['bash', str(SCRIPTS / 'macos-signing-keychain.sh'), 'cleanup'],
                       env=env, cwd=self.root, check=True, capture_output=True)
        self.assertFalse(work.exists())

    def test_private_material_is_temporary_and_cleanup_restores_keychains(self):
        env = dict(self.env, MACOS_CERTIFICATE_P12=base64.b64encode(b'fake-p12').decode(),
                   MACOS_CERTIFICATE_PASSWORD='fake-p12-password', APPLE_ID='test@example.invalid',
                   APPLE_APP_SPECIFIC_PASSWORD='fake-apple-password', APPLE_TEAM_ID='MOCKTEAM01')
        setup = subprocess.run(['bash', str(SCRIPTS / 'macos-signing-keychain.sh'), 'setup'],
                               env=env, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(setup.returncode, 0, setup.stderr)
        work = self.root / 'runner-temp/codeyard-apple-signing'
        self.assertFalse((work / 'certificate.p12').exists())
        exports = (self.root / 'github-env').read_text()
        self.assertIn('SIGN_IDENTITY=' + 'A' * 40, exports)
        for secret in ['fake-p12-password', 'fake-apple-password', env['MACOS_CERTIFICATE_P12']]:
            self.assertNotIn(secret, exports + setup.stdout + setup.stderr)
        subprocess.run(['bash', str(SCRIPTS / 'macos-signing-keychain.sh'), 'cleanup'],
                       env=env, cwd=self.root, check=True, capture_output=True)
        self.assertFalse(work.exists())
        self.assertIn(['security', 'list-keychains', '-d', 'user', '-s', '/original/login.keychain-db'], self.calls())


if __name__ == '__main__':
    unittest.main()
