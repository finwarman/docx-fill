// Random core templates and data from a seed. Every case is in docxtemplater's public tag syntax.

const NAMES = ["a", "b", "c", "l", "m", "x"];
const WORDS = ["hi", "x y", " ", "a&b", "<t>", "é", "—", "", "  sp  ", "tab\t"];
const STRINGS = ["S", "", "line1\nline2", "a & b", "<b>", "  pad ", "é😀", "0", "false", "\n"];
// Raw tags read `r`, whose values are well-formed paragraphs or not strings.
const RAW_VALUES = ["<w:p><w:r><w:t>R</w:t></w:r></w:p>", "", null, 3, true];
// Loop items other than objects are truthy: core answers every lookup on a falsy item with the item.
const LOOP_PRIMITIVES = ["S", 3, true, "é😀"];
const OPERANDS = ["a", "b", "c", "x", "a.b", "a.x.c", "l[0]", "l[1].a", "l.length", "m.length", "1", "2.5", "0", "'s'", "''", "true", "null", "$index", "this", "."];
const OPERATORS = ["+", "-", "*", "/", "%", "==", "!=", "===", "!==", "<", ">", "<=", ">=", "&&", "||"];
const DELIMITERS = [{ start: "{", end: "}" }, { start: "{", end: "}" }, { start: "[[", end: "]]" }, { start: "{{", end: "}}" }];

/**
 * @param {number} seed
 * @param {number} count
 * @param {{ expressions?: boolean }} options
 * @returns {{ id: string, body: string, data: object, options: object }[]}
 */
export function generateCases(seed, count, { expressions = false } = {}) {
  let state = seed >>> 0 || 1;
  const rand = () => (state = (Math.imul(state ^ (state >>> 15), 0x2c1b3c6d) + 0x6d2b79f5) >>> 0) / 2 ** 32;
  const int = (n) => Math.floor(rand() * n);
  const pick = (items) => items[int(items.length)];
  const chance = (p) => rand() < p;
  const escape = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  function value(depth) {
    switch (int(depth > 1 ? 6 : 9)) {
      case 0: case 1: return pick(STRINGS);
      case 2: return pick([0, 1, -2.5, 1e21, 3, 0.1, 7]);
      case 3: return pick([true, false]);
      case 4: return null;
      case 5: return pick([[], {}]);
      case 6: case 7: return Array.from({ length: int(4) }, () => (chance(0.8) ? object(depth + 1) : pick(LOOP_PRIMITIVES)));
      default: return object(depth + 1);
    }
  }
  function object(depth) {
    const result = {};
    for (const name of NAMES) if (chance(0.6)) result[name] = value(depth);
    if (chance(0.5)) result.r = pick(RAW_VALUES);
    return result;
  }

  // `.` stays apart from other symbols: core fails to compile `!.` or `.==1`.
  function expression(depth = 0) {
    const r = int(10);
    if (r < 4 || depth > 1) return pick(OPERANDS);
    if (r < 7) return `${expression(depth + 1)} ${pick(OPERATORS)} ${expression(depth + 1)}`;
    if (r < 8) return `!${pick(OPERANDS.filter((operand) => operand !== "."))}`;
    if (r < 9) return `(${expression(depth + 1)})`;
    return `${expression(depth + 1)} ? ${expression(depth + 1)} : ${expression(depth + 1)}`;
  }

  let delimiters = DELIMITERS[0];
  const tag = (inner) => delimiters.start + inner + delimiters.end;
  const tagName = () => (expressions && chance(0.7) ? expression() : pick(NAMES.concat(["."])));
  const stray = () => pick([delimiters.start, delimiters.end, delimiters.start + "a", "a" + delimiters.end, tag(""), tag(" a "), tag("#"), tag("/")]);

  // One paragraph's text: words, value tags and inline loops, closed by name or by `{/}`.
  function inline(depth) {
    const pieces = [];
    for (let i = 1 + int(4); i > 0; i--) {
      const r = int(10);
      if (r < 3) pieces.push(chance(0.05) ? stray() : pick(WORDS));
      else if (r < 7) pieces.push(tag(tagName()));
      else if (depth < 2) {
        const name = pick(NAMES);
        pieces.push(tag(pick(["#", "#", "^"]) + name), ...inline(depth + 1), tag("/" + (chance(0.8) ? name : "")));
      }
    }
    return pieces;
  }

  // Text split into runs at random points, often inside a tag, with run content between.
  function runs(text) {
    const cuts = new Set();
    for (let i = int(4); i > 0; i--) cuts.add(int(text.length + 1));
    const points = [0, ...[...cuts].sort((p, q) => p - q), text.length];
    let xml = "";
    for (let i = 0; i + 1 < points.length; i++) {
      const piece = text.slice(points[i], points[i + 1]);
      if (!piece && i) continue;
      const props = chance(0.3) ? "<w:rPr><w:b/></w:rPr>" : "";
      // A space inside an expression after `]` and `.` fails to compile in core, as `l[1]. a`.
      const extra = chance(0.15) ? pick(["<w:tab/>", "<w:br/>", ...(expressions ? [] : ['<w:t xml:space="preserve"> </w:t>'])]) : "";
      xml += `<w:r>${props}<w:t xml:space="preserve">${escape(piece)}</w:t>${extra}</w:r>`;
      if (chance(0.15)) xml += pick(['<w:proofErr w:type="spellStart"/>', '<w:bookmarkStart w:id="9" w:name="b"/><w:bookmarkEnd w:id="9"/>', "<w:r><w:t/></w:r>"]);
    }
    return xml;
  }
  const paragraph = (text) => `<w:p>${chance(0.2) ? '<w:pPr><w:jc w:val="center"/></w:pPr>' : ""}${runs(text)}</w:p>`;
  const cell = (text) => `<w:tc><w:p>${runs(text)}</w:p></w:tc>`;

  // Paragraphs, paragraph loops and tables with a row loop; a few loops are left unclosed.
  function blocks(depth) {
    const out = [];
    for (let i = 1 + int(3); i > 0; i--) {
      const r = int(10);
      if (r < 5) out.push(paragraph(inline(0).join("")));
      else if (r < 6) out.push(paragraph(tag("@r")));
      else if (r < 8 && depth < 2) {
        const name = pick(NAMES);
        const close = chance(0.95) ? [paragraph(tag("/" + name))] : [];
        out.push(paragraph(tag(pick(["#", "^"]) + name)), ...blocks(depth + 1), ...close);
      } else if (depth < 2) {
        const name = pick(NAMES);
        const loopRow = `<w:tr>${cell(tag("#" + name) + tag(tagName()))}${cell(tag(tagName()) + tag("/" + name))}</w:tr>`;
        const plainRow = `<w:tr>${cell(tag(tagName()))}${cell("h")}</w:tr>`;
        out.push(`<w:tbl><w:tblGrid><w:gridCol w:w="100"/><w:gridCol w:w="100"/></w:tblGrid>${chance(0.5) ? plainRow : ""}${loopRow}</w:tbl>`, paragraph(""));
      }
    }
    return out;
  }

  const cases = [];
  for (let i = 0; i < count; i++) {
    delimiters = pick(DELIMITERS);
    const options = { paragraphLoop: chance(0.8), linebreaks: chance(0.8), delimiters, ...(expressions ? { expressions: true } : {}) };
    cases.push({ id: `${seed}-${i}`, body: blocks(0).join(""), data: object(0), options });
  }
  return cases;
}
