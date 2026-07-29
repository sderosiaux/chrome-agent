const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

// The postinstall script is an ES module that installs on import; the env flag keeps it
// inert so its verification helpers can be exercised here.
process.env.CHROME_AGENT_POSTINSTALL_NOOP = '1';
const postinstallPath = path.resolve(__dirname, '..', '..', 'npm', 'scripts', 'postinstall.js');

describe('npm postinstall: the downloaded binary is checked against a published hash', () => {
  it('accepts the hash the release published', async () => {
    const { sha256File, verifyChecksum } = await import(`file://${postinstallPath}`);
    const file = path.join(os.tmpdir(), `chrome-agent-postinstall-${process.pid}.bin`);
    fs.writeFileSync(file, 'pretend this is a native binary');
    const actual = sha256File(file);
    fs.rmSync(file, { force: true });

    // Exactly as `shasum -a 256 | awk '{print $1}'` writes it, and with the trailing
    // newline a file read keeps.
    assert.doesNotThrow(() => verifyChecksum(actual, `${actual}\n`, 'chrome-agent-linux-x64'));
    // And the two-column `shasum` form, in case the release step ever stops trimming.
    assert.doesNotThrow(() => verifyChecksum(actual, `${actual}  chrome-agent-linux-x64\n`, 'x'));
  });

  it('refuses bytes that do not match', async () => {
    const { verifyChecksum } = await import(`file://${postinstallPath}`);
    const a = 'a'.repeat(64);
    const b = 'b'.repeat(64);
    assert.throws(() => verifyChecksum(a, b, 'chrome-agent-linux-x64'), /Checksum mismatch/);
  });

  it('refuses a checksum file that is not a checksum', async () => {
    const { verifyChecksum } = await import(`file://${postinstallPath}`);
    // A 404 page, an empty file, or a truncated hash must fail closed — accepting them
    // would make the whole check decorative.
    for (const bad of ['', '\n', 'Not Found', 'abc123', 'z'.repeat(64)]) {
      assert.throws(() => verifyChecksum('a'.repeat(64), bad, 'x'), /Malformed checksum/);
    }
  });
});
