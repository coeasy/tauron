// 极简 tar.gz 读取器：只依赖 node:zlib，用来校验 npm / cargo 的打包产物内部内容。
// 支持 ustar、GNU longname（typeflag 'L'）、PAX 扩展头（typeflag 'x'），
// 因为 npm/pnpm 生成的 tarball 对超长路径会走 PAX 扩展头。
import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';

function parseOctal(buf) {
  const s = buf.toString('ascii').replace(/\0.*$/, '').trim();
  return s ? parseInt(s, 8) : 0;
}

/**
 * 读取 .tar.gz（或未压缩的 .tar），返回 [{ path, content }]。
 * content 为 null 表示目录条目。路径已去掉前导 "./"。
 */
export function readTarball(file) {
  const raw = readFileSync(file);
  // gzip magic: 1f 8b
  const tar = raw[0] === 0x1f && raw[1] === 0x8b ? gunzipSync(raw) : raw;
  const entries = [];
  let offset = 0;
  let pendingLongName = null;
  let pendingPax = null;

  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break; // 两个全零块表示归档结束

    let name = header.subarray(0, 100).toString('utf8').replace(/\0.*$/, '');
    const prefix = header.subarray(345, 500).toString('utf8').replace(/\0.*$/, '');
    if (prefix) name = `${prefix}/${name}`;

    const size = parseOctal(header.subarray(124, 136));
    const type = String.fromCharCode(header[156]) || '0';
    const bodyStart = offset + 512;
    const body = tar.subarray(bodyStart, bodyStart + size);

    if (type === 'L') {
      pendingLongName = body.toString('utf8').replace(/\0.*$/, '');
    } else if (type === 'x') {
      pendingPax = {};
      for (const line of body.toString('utf8').split('\n')) {
        const eq = line.indexOf('=');
        if (eq > 0) pendingPax[line.slice(0, eq).trim()] = line.slice(eq + 1);
      }
    } else if (type === '0' || type === '5') {
      let real = name;
      if (pendingPax?.path) real = pendingPax.path;
      else if (pendingLongName) real = pendingLongName;
      pendingLongName = null;
      pendingPax = null;
      entries.push({
        path: real.replace(/^\.\//, ''),
        content: type === '0' ? Buffer.from(body) : null,
      });
    }
    offset = bodyStart + Math.ceil(size / 512) * 512;
  }
  return entries;
}
