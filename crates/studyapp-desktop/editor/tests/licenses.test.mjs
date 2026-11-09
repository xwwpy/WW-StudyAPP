import test from 'node:test';
import assert from 'node:assert/strict';
import { licenseNotices } from '../license-notices.mjs';

test('license output has stable LF bytes for Windows and Unix package sources', () => {
  const unix=['zip@1\nLicense: MIT\n\nCopyright ZIP\n','alpha@2\nLicense: Apache-2.0\n\nCopyright A\n'];
  const windows=unix.toReversed().map(value=>value.replaceAll('\n','\r\n'));
  assert.equal(licenseNotices(windows),licenseNotices(unix));
  assert.ok(!licenseNotices(windows).includes('\r'));
});

test('license order ignores installation roots and retains distinct package versions', () => {
  const windows=new Map([['C:\notes\node_modules\zip','zip@2\nMIT'],['C:\notes\node_modules\alpha','alpha@1\nISC']]);
  const unix=new Map([['/home/runner/node_modules/alpha','alpha@1\nISC'],['/home/runner/node_modules/zip','zip@2\nMIT']]);
  assert.equal(licenseNotices(windows.values()),licenseNotices(unix.values()));
  assert.ok(licenseNotices(['zip@1\nMIT','zip@2\nMIT']).includes('zip@1'));
  assert.ok(licenseNotices(['zip@1\nMIT','zip@2\nMIT']).includes('zip@2'));
});

test('duplicate notices are removed only when normalized texts match exactly', () => {
  assert.equal(licenseNotices(['zip@1\r\nCopyright A','zip@1\nCopyright A']),'zip@1\nCopyright A');
  const text=licenseNotices(['zip@1\nCopyright A','zip@1\nCopyright B']);
  assert.ok(text.includes('Copyright A')&&text.includes('Copyright B'));
});
