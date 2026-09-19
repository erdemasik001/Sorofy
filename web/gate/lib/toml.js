/**
 * Just enough TOML to read a `stellar.toml`.
 *
 * SEP-1 files are a small, flat dialect: top-level `KEY = value`, `[TABLE]` sections, and
 * `[[CURRENCIES]]` array-of-table entries. That is the whole grammar this needs, so it is
 * ~60 lines here rather than a dependency — the same trade the Sorofy explorer makes, and it
 * keeps the gate's only two run-time imports the ones that genuinely cannot be hand-rolled.
 *
 * It is strict about what it returns and quiet about what it does not understand: an
 * unrecognised line is skipped, never guessed at. A SEP-1 file that grows a construct this
 * cannot read will be missing a field, which the caller checks for, rather than carrying a
 * value this invented.
 */

/** Parse a `stellar.toml` into `{ ...topLevel, TABLE: {...}, CURRENCIES: [...] }`. */
export function parseToml(text) {
  const root = {};
  let target = root;

  for (const rawLine of String(text).split(/\r?\n/)) {
    const line = stripComment(rawLine).trim();
    if (!line) continue;

    // [[ARRAY_OF_TABLES]] — a new entry appended to that key.
    const arrayTable = /^\[\[\s*([A-Za-z0-9_.-]+)\s*\]\]$/.exec(line);
    if (arrayTable) {
      const key = arrayTable[1];
      if (!Array.isArray(root[key])) root[key] = [];
      target = {};
      root[key].push(target);
      continue;
    }

    // [TABLE]
    const table = /^\[\s*([A-Za-z0-9_.-]+)\s*\]$/.exec(line);
    if (table) {
      const key = table[1];
      if (typeof root[key] !== 'object' || root[key] === null || Array.isArray(root[key])) {
        root[key] = {};
      }
      target = root[key];
      continue;
    }

    const pair = /^([A-Za-z0-9_.-]+)\s*=\s*(.+)$/.exec(line);
    if (pair) target[pair[1]] = parseValue(pair[2].trim());
  }
  return root;
}

/**
 * Drop a trailing `#` comment, but not one inside a quoted string — `desc` fields routinely
 * contain `#`, and truncating one would silently corrupt the value.
 */
function stripComment(line) {
  let quote = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === '#') {
      return line.slice(0, i);
    }
  }
  return line;
}

function parseValue(raw) {
  if (raw.startsWith('[')) {
    const inner = raw.slice(1, raw.lastIndexOf(']'));
    return splitTopLevel(inner)
      .map((part) => part.trim())
      .filter(Boolean)
      .map(parseValue);
  }
  if (raw.startsWith('"') || raw.startsWith("'")) return unquote(raw);
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  if (/^-?\d+$/.test(raw)) return Number(raw);
  if (/^-?\d*\.\d+$/.test(raw)) return Number(raw);
  return raw;
}

/** Split a list body on commas that are not inside a quoted string. */
function splitTopLevel(inner) {
  const parts = [];
  let depth = 0;
  let quote = null;
  let at = 0;
  for (let i = 0; i < inner.length; i++) {
    const c = inner[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '[') depth++;
    else if (c === ']') depth--;
    else if (c === ',' && depth === 0) {
      parts.push(inner.slice(at, i));
      at = i + 1;
    }
  }
  parts.push(inner.slice(at));
  return parts;
}

function unquote(raw) {
  const q = raw[0];
  let out = '';
  for (let i = 1; i < raw.length; i++) {
    const c = raw[i];
    if (c === q) break;
    if (c === '\\' && q === '"') {
      const next = raw[++i];
      out += { n: '\n', t: '\t', r: '\r', '"': '"', '\\': '\\' }[next] ?? next;
    } else {
      out += c;
    }
  }
  return out;
}
