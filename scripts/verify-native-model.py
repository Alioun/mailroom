#!/usr/bin/env python3
"""Compile and run the existing pure Swift model tests without SwiftPM."""

import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys


def main():
    if len(sys.argv) != 2:
        print('status=expected_verification_directory')
        return 64
    root = Path(__file__).resolve().parent.parent
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    package = root / 'native/macos'
    test_source = package / 'Tests/MailroomNativeTests/ExactTests.swift'
    cases = re.findall(r'func (test\w+)\(\) throws', test_source.read_text())
    if not cases or len(cases) != len(set(cases)):
        print('status=invalid_model_test_discovery')
        return 65
    resources = output / 'ModelFixtures.bundle/Contents/Resources'
    resources.mkdir(parents=True, exist_ok=True)
    (resources.parent / 'Info.plist').write_bytes(plistlib.dumps({
        'CFBundleIdentifier': 'local.mailroom.model-fixtures',
        'CFBundlePackageType': 'BNDL',
        'CFBundleVersion': '1',
    }))
    (resources / 'exact-actions.json').write_bytes((test_source.parent / 'exact-actions.json').read_bytes())
    bundle_literal = json.dumps(str(resources.parent.parent))
    (output / 'Resources.swift').write_text('import Foundation\nextension Bundle { static var module: Bundle { Bundle(path: ' + bundle_literal + ')! } }\n')
    (output / 'main.swift').write_text(
        'import XCTest\nimport Darwin\n'
        'let suite = ExactTests.defaultTestSuite\n'
        f'guard suite.testCaseCount == {len(cases)} else {{ exit(65) }}\n'
        'suite.run()\n'
        'guard let result = suite.testRun, result.executionCount == suite.testCaseCount, '
        'result.hasSucceeded else { exit(1) }\n'
    )
    for directory in ['home', 'temp', 'module-cache']:
        (output / directory).mkdir(exist_ok=True)
    environment = {
        'PATH': '/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin',
        'HOME': str(output / 'home'),
        'TMPDIR': str(output / 'temp'),
    }
    framework = Path('/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/Library/Frameworks')
    swift_support = framework.parent.parent / 'usr/lib'
    if not framework.is_dir() or not (swift_support / 'XCTest.swiftmodule').is_dir():
        print('status=xctest_framework_unavailable')
        return 69
    cache = ['-module-cache-path', str(output / 'module-cache')]
    stages = [
        ('core', ['/usr/bin/swiftc', '-enable-testing', '-emit-module', '-emit-library', '-module-name', 'MailroomNative', '-emit-module-path', str(output / 'MailroomNative.swiftmodule'), *cache, str(package / 'Sources/MailroomNative/Exact.swift'), str(package / 'Sources/MailroomNative/Pipe.swift'), '-o', str(output / 'libMailroomNative.dylib')]),
        ('test_build', ['/usr/bin/swiftc', '-I', str(output), '-I', str(swift_support), '-L', str(output), '-L', str(swift_support), '-lMailroomNative', '-lXCTestSwiftSupport', '-F', str(framework), '-Xlinker', '-rpath', '-Xlinker', str(output), '-Xlinker', '-rpath', '-Xlinker', str(framework), '-Xlinker', '-rpath', '-Xlinker', str(swift_support), *cache, str(test_source), str(output / 'Resources.swift'), str(output / 'main.swift'), '-o', str(output / 'model-tests')]),
        ('tests', [str(output / 'model-tests')]),
    ]
    results = []
    for stage, command in stages:
        with (output / f'{stage}.log').open('wb') as log:
            try:
                result = subprocess.run(command, env=environment, cwd=root, stdout=log, stderr=subprocess.STDOUT, timeout=180, check=False)
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 124
        results.append({'stage': stage, 'exit_code': code})
        if code != 0:
            (output / 'result.json').write_text(json.dumps({'status': 'failed', 'stages': results}, indent=2) + '\n')
            print(json.dumps({'status': 'failed', 'stage': stage, 'exit_code': code}))
            return 1
    record = {'status': 'passed', 'tests': cases, 'stages': results, 'swiftpm_used': False, 'native_helper_invoked': False}
    (output / 'result.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({'status': 'passed', 'tests': len(cases), 'native_helper_invoked': False}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
