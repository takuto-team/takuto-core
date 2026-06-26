/**
 * Minimal TOML reader for asserting the `config.toml` Takuto writes on
 * completion (`/etc/takuto/config.toml`, §6). It is deliberately small — it
 * covers exactly what the onboarding specs read back: dotted section headers
 * (`[agent.providers.opencode]`), scalar `key = value` pairs (string / integer /
 * float / boolean) and string arrays — both single-line (`extra_args = ["--foo"]`)
 * and the multi-line form `toml::to_string_pretty` emits (one element per line
 * with a trailing comma), which is how Takuto's `ConfigWriter` serializes
 * `cors_origins`, `available_providers`, etc.
 * It is not a general-purpose TOML parser; multi-line strings and inline tables
 * are out of scope because the writer never emits them for the fields under test.
 */

export type TomlValue = string | number | boolean | TomlValue[] | TomlTable;

export interface TomlTable {
  [key: string]: TomlValue;
}

/** Strip a `#` comment that is not inside a string literal. */
function stripComment(line: string): string {
  let inString = false;
  let quote = '';
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (inString) {
      if (ch === quote) {
        inString = false;
      }
    } else if (ch === '"' || ch === "'") {
      inString = true;
      quote = ch;
    } else if (ch === '#') {
      return line.slice(0, i);
    }
  }
  return line;
}

/** Parse a single TOML scalar / single-line array value. */
function parseValue(raw: string): TomlValue {
  const text = raw.trim();
  if (text.startsWith('[') && text.endsWith(']')) {
    const inner = text.slice(1, -1).trim();
    if (inner === '') {
      return [];
    }
    return splitTopLevel(inner).map((item) => parseValue(item));
  }
  if (
    (text.startsWith('"') && text.endsWith('"')) ||
    (text.startsWith("'") && text.endsWith("'"))
  ) {
    return text.slice(1, -1);
  }
  if (text === 'true') {
    return true;
  }
  if (text === 'false') {
    return false;
  }
  const num = Number(text);
  if (text !== '' && !Number.isNaN(num)) {
    return num;
  }
  return text;
}

/** Split a comma-separated array body, honouring quoted commas. */
function splitTopLevel(body: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let inString = false;
  let quote = '';
  let current = '';
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i];
    if (inString) {
      if (ch === quote) {
        inString = false;
      }
      current += ch;
      continue;
    }
    if (ch === '"' || ch === "'") {
      inString = true;
      quote = ch;
      current += ch;
    } else if (ch === '[') {
      depth += 1;
      current += ch;
    } else if (ch === ']') {
      depth -= 1;
      current += ch;
    } else if (ch === ',' && depth === 0) {
      parts.push(current.trim());
      current = '';
    } else {
      current += ch;
    }
  }
  if (current.trim() !== '') {
    parts.push(current.trim());
  }
  return parts;
}

/** Net `[`-minus-`]` bracket depth of `text`, ignoring brackets in strings. */
function bracketDepth(text: string): number {
  let depth = 0;
  let inString = false;
  let quote = '';
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (inString) {
      if (ch === quote) {
        inString = false;
      }
    } else if (ch === '"' || ch === "'") {
      inString = true;
      quote = ch;
    } else if (ch === '[') {
      depth += 1;
    } else if (ch === ']') {
      depth -= 1;
    }
  }
  return depth;
}

/** Resolve (creating as needed) the nested table addressed by a dotted path. */
function tableAt(root: TomlTable, path: string[]): TomlTable {
  let cursor = root;
  for (const key of path) {
    const existing = cursor[key];
    if (existing && typeof existing === 'object' && !Array.isArray(existing)) {
      cursor = existing;
    } else {
      const created: TomlTable = {};
      cursor[key] = created;
      cursor = created;
    }
  }
  return cursor;
}

/** Split a dotted header/key into segments, unquoting quoted segments. */
function splitPath(header: string): string[] {
  return header
    .split('.')
    .map((seg) => seg.trim())
    .map((seg) =>
      (seg.startsWith('"') && seg.endsWith('"')) ||
      (seg.startsWith("'") && seg.endsWith("'"))
        ? seg.slice(1, -1)
        : seg,
    );
}

/** Parse a TOML document into a nested table. */
export function parseToml(input: string): TomlTable {
  const root: TomlTable = {};
  let current = root;
  const lines = input.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const line = stripComment(lines[i]).trim();
    if (line === '') {
      continue;
    }
    // A section header is a bracketed line with no `=` before the bracket;
    // distinguish it from an array value (`key = [ … ]`).
    if (line.startsWith('[') && line.endsWith(']') && bracketDepth(line) === 0) {
      const header = line.slice(1, -1).trim();
      current = tableAt(root, splitPath(header));
      continue;
    }
    const eq = line.indexOf('=');
    if (eq === -1) {
      continue;
    }
    const key = line.slice(0, eq).trim();
    let valueText = line.slice(eq + 1).trim();
    // Multi-line array: keep consuming lines until the brackets balance.
    while (bracketDepth(valueText) > 0 && i + 1 < lines.length) {
      i += 1;
      valueText += `\n${stripComment(lines[i]).trim()}`;
    }
    const value = parseValue(valueText);
    const path = splitPath(key);
    const leaf = path.pop();
    if (leaf === undefined) {
      continue;
    }
    const target = path.length > 0 ? tableAt(current, path) : current;
    target[leaf] = value;
  }
  return root;
}
