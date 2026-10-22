// Regenerate the macOS icon from its editable SVG. Requires sharp and macOS iconutil.
const sharp = require('sharp');
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

async function main() {
  const assets = path.resolve(__dirname, '../crates/app/assets');
  const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'del-icon-'));
  const iconset = path.join(temp, 'Del.iconset');
  try {
    await fs.mkdir(iconset);
    const svg = await fs.readFile(path.join(assets, 'del.svg'));
    await sharp(svg).png().toFile(path.join(assets, 'del.png'));
    for (const size of [16, 32, 128, 256, 512]) {
      for (const scale of [1, 2]) {
        const name = `icon_${size}x${size}${scale === 2 ? '@2x' : ''}.png`;
        await sharp(svg).resize(size * scale).png().toFile(path.join(iconset, name));
      }
    }
    execFileSync('/usr/bin/iconutil', ['-c', 'icns', iconset, '-o', path.join(assets, 'Del.icns')]);
  } finally {
    await fs.rm(temp, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
