// Black-box runs of the MIT docxtemplater core, the reference the differential tests compare with.
import PizZip from "pizzip";
import Docxtemplater from "docxtemplater";
import expressionParser from "docxtemplater/expressions.js";

const CONTENT_TYPES = `<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>`;
const RELATIONSHIPS = `<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>`;

/**
 * Fills one case with core. A missing value prints nothing, as docx-fill prints it.
 * @returns {{ out: string } | { error: string }}
 */
export function fillWithCore({ body, data, options: { expressions, ...options } }) {
  const zip = new PizZip();
  zip.file("[Content_Types].xml", CONTENT_TYPES);
  zip.file("_rels/.rels", RELATIONSHIPS);
  zip.file("word/document.xml", `<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${body}</w:body></w:document>`);
  try {
    const document = new Docxtemplater(zip, { ...options, errorLogging: false, nullGetter: () => "", ...(expressions ? { parser: expressionParser } : {}) });
    document.render(data);
    return { out: document.getZip().file("word/document.xml").asText() };
  } catch (error) {
    const errors = error.properties?.errors ?? [error];
    return { error: errors.map((each) => `${each.properties?.id}: ${each.message}`).join("; ") };
  }
}
