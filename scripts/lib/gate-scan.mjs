#!/usr/bin/env node
// Shared plumbing for the narrow mechanical source gates (V7 §9).
//
// These gates are deliberately NOT parsers. Each one states a small set of
// ordering / presence rules over named source blocks so that a specific,
// already-fixed defect class cannot silently come back. Anything harder belongs
// in a Rust test, not here.

import { readFileSync, readdirSync, statSync } from 'node:fs';

export function collectFiles(roots, extensions) {
  const files = [];
  const walk = (path) => {
    for (const name of readdirSync(path)) {
      const item = `${path}/${name}`;
      const stat = statSync(item);
      if (stat.isDirectory()) walk(item);
      else if (extensions.some((ext) => item.endsWith(ext))) files.push(item);
    }
  };
  for (const root of roots) walk(root);
  return files.sort();
}

export function linesOf(file) {
  return readFileSync(file, 'utf8').split(/\r?\n/);
}

/// Blank out comment bodies and string/char/template-literal contents so brace
/// counting cannot be fooled by `format!("{} …")`, a `"//"` inside a message, or
/// a TS `` `…success: true…` `` template string. The original lines stay
/// available for the actual rule matching — this projection is only used to find
/// block boundaries and to match code-level needles.
///
/// Interpolation (`${…}`) is blanked along with the rest of the literal: a gate
/// that needs an interpolated value must match the raw line instead.
export function stripped(lines) {
  return lines.map((line) => {
    let out = '';
    let inString = false;
    let inChar = false;
    let inTemplate = false;
    for (let i = 0; i < line.length; i += 1) {
      const ch = line[i];
      const next = line[i + 1];
      if (!inString && !inChar && !inTemplate && ch === '/' && next === '/') break;
      if (ch === '`' && !inString && !inChar) {
        inTemplate = !inTemplate;
        out += ' ';
        continue;
      }
      if (inTemplate) {
        out += ' ';
        continue;
      }
      if (ch === '"' && !inChar) {
        if (inString && line[i - 1] !== '\\') inString = false;
        else if (!inString) inString = true;
        out += ' ';
        continue;
      }
      if (ch === "'" && !inString) {
        if (inChar && line[i - 1] !== '\\') inChar = false;
        else if (!inChar) inChar = true;
        out += ' ';
        continue;
      }
      out += inString || inChar ? ' ' : ch;
    }
    return out;
  });
}

/// Index of the first line at/after `from` whose stripped projection contains
/// `needle`, or -1.
export function findLine(lines, needle, from = 0) {
  const projection = stripped(lines);
  for (let i = from; i < projection.length; i += 1) {
    if (projection[i].includes(needle)) return i;
  }
  return -1;
}

/// Brace-matched block that starts at the first `{` on/after `from`.
/// Returns `{ start, end, body }` with inclusive line indices, or null.
export function blockAt(rawLines, from) {
  const projection = stripped(rawLines);
  let start = -1;
  for (let i = from; i < projection.length; i += 1) {
    if (projection[i].includes('{')) {
      start = i;
      break;
    }
  }
  if (start === -1) return null;
  let depth = 0;
  for (let i = start; i < projection.length; i += 1) {
    for (const ch of projection[i]) {
      if (ch === '{') depth += 1;
      else if (ch === '}') depth -= 1;
    }
    if (depth <= 0) {
      return { start, end: i, body: rawLines.slice(start, i + 1) };
    }
  }
  return null;
}

/// Find a top-level definition whose header matches `headerPattern` and return
/// `{ headerIndex, header, ...blockAt }`.
export function findBlock(rawLines, headerPattern) {
  const projection = stripped(rawLines);
  for (let i = 0; i < projection.length; i += 1) {
    const match = projection[i].match(headerPattern);
    if (!match) continue;
    const block = blockAt(rawLines, i);
    if (!block) continue;
    return { ...block, header: rawLines[i], headerIndex: i, name: match[1] ?? '' };
  }
  return null;
}

export function finish(gateName, failures, fileCount) {
  if (failures.length > 0) {
    console.error(`${gateName} gate failed:`);
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(`${gateName} gate OK across ${fileCount} source files.`);
}
