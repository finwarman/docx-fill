// Fills random templates with core and with docx-fill and compares them:
//   node run.mjs [--seed 1] [--count 1000] [--expressions] [--show 3]
// Exits 1 when a difference is not a known one. DOCX_FILL_RUNNER replaces the cargo command.
import { execSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { generateCases } from "./generate.mjs";
import { fillWithCore } from "./core.mjs";

const { values: args } = parseArgs({ options: { seed: { type: "string", default: "1" }, count: { type: "string", default: "1000" }, expressions: { type: "boolean", default: false }, show: { type: "string", default: "3" } } });
const RUNNER = process.env.DOCX_FILL_RUNNER ?? "cargo run --quiet --release --example differential_fill --";

// Differences left as they are, each with why.
const Known = {
  RawNonString: "core throws on a non-string raw value; docx-fill prints nothing, as its render cannot fail",
  TagAcrossParagraphs: "core reads a tag across paragraphs and drops the XML between, which can corrupt the document; docx-fill refuses it",
};

const cases = generateCases(Number(args.seed), Number(args.count), { expressions: args.expressions });
const path = join(mkdtempSync(join(tmpdir(), "docx-fill-differential-")), "cases.json");
writeFileSync(path, JSON.stringify(cases));
const core = cases.map(fillWithCore);
const fill = execSync(`${RUNNER} ${JSON.stringify(path)}`, { cwd: new URL("..", import.meta.url), maxBuffer: 1 << 30, encoding: "utf8" }).trim().split("\n").map((line) => JSON.parse(line));

const unescape = (text) => text.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");

/** Paragraphs, tables, rows and cells as brackets around their text, breaks and tabs. */
function shape(xml) {
  const body = xml.replace(/^[\s\S]*<w:body>|<\/w:body>[\s\S]*$/g, "");
  let out = "";
  let isInText = false;
  for (const [, close, name, rest, text] of body.matchAll(/<(\/?)(w:[A-Za-z]+)([^>]*)>|([^<]+)/g)) {
    if (text !== undefined) {
      if (isInText) out += unescape(text);
    } else if (name === "w:t") {
      isInText = !close && !rest.endsWith("/");
    } else if (["w:p", "w:tbl", "w:tr", "w:tc"].includes(name)) {
      out += close ? "]" : rest.endsWith("/") ? `<${name.slice(2)}/>` : `${name.slice(2)}[`;
    } else if (!close && name === "w:br") {
      out += "⏎";
    } else if (!close && name === "w:tab") {
      out += "⇥";
    }
  }
  return out;
}

/** Whether a paragraph of the template opens a tag it does not close. */
function hasTagAcrossParagraphs({ body, options: { delimiters } }) {
  return [...body.matchAll(/<w:p>([\s\S]*?)<\/w:p>/g)].some(([, paragraph]) => {
    const text = unescape([...paragraph.matchAll(/<w:t[^>]*>([^<]*)<\/w:t>/g)].map(([, part]) => part).join(""));
    return text.lastIndexOf(delimiters.start) > text.lastIndexOf(delimiters.end);
  });
}

/** Why core and docx-fill differ on a case, or `undefined` when they agree. */
function compare(each, core, fill) {
  if (fill.error === "panic") return "docx-fill panics";
  if (core.error && fill.error) return undefined;
  if (core.error?.startsWith("invalid_raw_xml_value")) return Known.RawNonString;
  if (fill.error?.endsWith("is unclosed") && hasTagAcrossParagraphs(each)) return Known.TagAcrossParagraphs;
  if (core.error) return "core refuses the template, docx-fill fills it";
  if (fill.error) return "docx-fill refuses the template, core fills it";
  return shape(core.out) === shape(fill.out) ? undefined : "the filled text differs";
}

const groups = new Map();
cases.forEach((each, index) => {
  const reason = compare(each, core[index], fill[index]);
  if (!reason) return;
  if (!groups.has(reason)) groups.set(reason, []);
  groups.get(reason).push(index);
});

const known = new Set(Object.values(Known));
let unknown = 0;
for (const [reason, indexes] of groups) {
  const isKnown = known.has(reason);
  if (!isKnown) unknown += indexes.length;
  console.log(`${isKnown ? "known" : "DIFFERS"}: ${reason}: ${indexes.length}`);
  if (isKnown) continue;
  indexes.sort((p, q) => cases[p].body.length - cases[q].body.length);
  for (const index of indexes.slice(0, Number(args.show))) {
    const { id, body, data, options } = cases[index];
    console.log(`  case ${id} ${JSON.stringify(options)}\n  template: ${shape(body)}\n  data: ${JSON.stringify(data)}`);
    console.log(`  core:      ${core[index].error ?? shape(core[index].out)}\n  docx-fill: ${fill[index].error ?? shape(fill[index].out)}`);
  }
}
const same = cases.length - [...groups.values()].reduce((total, indexes) => total + indexes.length, 0);
console.log(`${same} of ${cases.length} cases agree, ${unknown} differ in an unknown way (seed ${args.seed}${args.expressions ? ", expressions" : ""})`);
process.exitCode = unknown ? 1 : 0;
