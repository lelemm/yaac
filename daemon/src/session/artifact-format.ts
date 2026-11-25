// Validate generated document formats before adding their immutable bytes to the upload store.
import { unzipSync } from "fflate";

const office = {
  docx: ["word/document.xml", "wordprocessingml.document"],
  xlsx: ["xl/workbook.xml", "spreadsheetml.sheet"],
  pptx: ["ppt/presentation.xml", "presentationml.presentation"],
} as const;

export function artifactFormat(ext: string, bytes: Buffer, signature: string | null): string {
  const prefix = "application/vnd.openxmlformats-officedocument.";
  if (Object.hasOwn(office, ext)) {
    const [part, type] = office[ext as keyof typeof office];
    let hasPart = false;
    let manifestSeen = false;
    let entries = 0;
    // Inspect directory entries, inflating only the small content-type manifest.
    const files = unzipSync(bytes, { filter(entry) {
      if (++entries > 8192) throw new Error("document package has too many entries");
      if (/vbaproject\.bin$/i.test(entry.name)) throw new Error("macro-enabled documents are unsupported");
      if (entry.name === part) hasPart = true;
      if (entry.name !== "[Content_Types].xml") return false;
      if (manifestSeen || Math.max(entry.size, entry.originalSize) > 256 * 1024) throw new Error("invalid document manifest");
      manifestSeen = true;
      return true;
    } });
    const manifest = Buffer.from(files["[Content_Types].xml"] ?? []).toString("utf8");
    if (!hasPart || !manifest.includes(prefix + type + ".main+xml") || /macroenabled|vbaproject/i.test(manifest)) {
      throw new Error("document contents do not match its extension");
    }
    return prefix + type;
  }
  const types = new Map(Object.entries({
    md: "text/markdown", markdown: "text/markdown", txt: "text/plain", mmd: "text/plain", mermaid: "text/plain",
    html: "text/html", htm: "text/html",
    csv: "text/csv", svg: "image/svg+xml", pdf: "application/pdf", png: "image/png", gif: "image/gif",
    webp: "image/webp", jpg: "image/jpeg", jpeg: "image/jpeg",
  }));
  const type = types.get(ext);
  if (!type) throw new Error("unsupported artifact format");
  if (type.startsWith("text/") || ext === "svg") {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (!text.length || bytes.includes(0)) throw new Error("expected UTF-8 text");
    if (ext === "svg" && !/^\s*(?:<\?xml[^?]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*<svg(?:\s|\/?>)/u.test(text)) throw new Error("expected an SVG document");
  } else if (signature !== type) throw new Error("artifact contents do not match its extension");
  return type;
}
