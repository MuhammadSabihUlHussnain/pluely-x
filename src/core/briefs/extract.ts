// Brief file → plain text: PDF (pdfjs-dist), DOCX (mammoth), TXT/MD.
// Libraries load on demand so they never weigh on the overlay window.

export const BRIEF_ACCEPT = ".pdf,.docx,.txt,.md,.markdown,text/plain,application/pdf";

export interface ExtractResult {
  text: string;
  name: string;
  size: number;
}

function tidy(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

async function extractPdf(data: ArrayBuffer): Promise<string> {
  const pdfjs = await import("pdfjs-dist");
  const worker = await import("pdfjs-dist/build/pdf.worker.min.mjs?url");
  pdfjs.GlobalWorkerOptions.workerSrc = worker.default;
  const task = pdfjs.getDocument({ data: new Uint8Array(data) });
  const doc = await task.promise;
  const pages: string[] = [];
  for (let i = 1; i <= doc.numPages; i++) {
    const page = await doc.getPage(i);
    const content = await page.getTextContent();
    let line = "";
    const lines: string[] = [];
    for (const item of content.items) {
      if (!("str" in item)) continue;
      line += item.str;
      if (item.hasEOL) {
        lines.push(line);
        line = "";
      }
    }
    if (line) lines.push(line);
    pages.push(lines.join("\n"));
  }
  await task.destroy();
  return pages.join("\n\n");
}

async function extractDocx(data: ArrayBuffer): Promise<string> {
  const mammoth = await import("mammoth");
  const res = await mammoth.extractRawText({ arrayBuffer: data });
  return res.value;
}

export async function extractBriefText(file: File): Promise<ExtractResult> {
  const name = file.name;
  const ext = name.toLowerCase().split(".").pop() ?? "";
  let text: string;
  if (ext === "pdf" || file.type === "application/pdf") {
    try {
      text = await extractPdf(await file.arrayBuffer());
    } catch {
      throw new Error(`“${name}” couldn't be read as a PDF. It may be damaged or password-protected.`);
    }
  } else if (ext === "docx") {
    try {
      text = await extractDocx(await file.arrayBuffer());
    } catch {
      throw new Error(`“${name}” couldn't be read as a Word document. Save it as .docx or PDF and try again.`);
    }
  } else if (["txt", "md", "markdown", "text"].includes(ext) || file.type.startsWith("text/")) {
    text = await file.text();
  } else {
    throw new Error(`“${name}” isn't a PDF, DOCX, TXT or MD file.`);
  }
  text = tidy(text);
  if (!text) throw new Error(`No text found in “${name}”. Scanned PDFs need OCR first.`);
  return { text, name, size: file.size };
}
