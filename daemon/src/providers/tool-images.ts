import * as fs from "node:fs";
import * as path from "node:path";
import type { ToolImage } from "../protocol/generated/types.js";

const MAX_BYTES = 32 * 1024 * 1024;
export type StoreToolImage = (bytes: Buffer, name: string) => ToolImage | null;

function save(store: StoreToolImage, bytes: Buffer, name: string): ToolImage | null {
  try {
    return store(bytes, name);
  } catch {
    return null;
  }
}

export function base64Image(store: StoreToolImage | undefined, data: unknown, name: string): ToolImage | null {
  if (!store || typeof data !== "string" || data.length > Math.ceil(MAX_BYTES / 3) * 4) return null;
  if (data.length % 4 === 1 || !/^[A-Za-z0-9+/]+={0,2}$/.test(data)) return null;
  const bytes = Buffer.from(data, "base64");
  return bytes.length <= MAX_BYTES ? save(store, bytes, name) : null;
}

export function imagePath(store: StoreToolImage | undefined, file: unknown): ToolImage | null {
  if (!store || typeof file !== "string" || !path.isAbsolute(file)) return null;
  try {
    const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK);
    try {
      const stat = fs.fstatSync(fd);
      if (!stat.isFile() || stat.size === 0 || stat.size > MAX_BYTES) return null;
      const bytes = Buffer.alloc(stat.size);
      let offset = 0;
      while (offset < bytes.length) {
        const n = fs.readSync(fd, bytes, offset, bytes.length - offset, offset);
        if (n === 0) return null;
        offset += n;
      }
      return save(store, bytes, path.basename(file));
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return null;
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

/** MCP can also embed a binary image as a resource. */
function imageResource(block: Record<string, unknown>): Record<string, unknown> | null {
  const resource = record(block.resource);
  return block.type === "resource" && typeof resource.mimeType === "string" &&
    /^image\//i.test(resource.mimeType) && typeof resource.blob === "string" ? resource : null;
}

export function isImageBlock(value: unknown): boolean {
  const block = record(value);
  return block.type === "image" || block.type === "inputImage" || block.type === "input_image" || imageResource(block) !== null;
}

function dataUrlImage(store: StoreToolImage, url: unknown, name: string): ToolImage | null {
  if (typeof url !== "string") return null;
  const prefix = /^data:image\/(?:png|jpeg|gif|webp);base64,/i.exec(url);
  return prefix ? base64Image(store, url.slice(prefix[0].length), name) : null;
}

/** Claude and Codex tool results can carry inline images. Only data URLs are decoded. */
export function resultImages(store: StoreToolImage | undefined, content: unknown): ToolImage[] {
  if (!store || !Array.isArray(content)) return [];
  const images: ToolImage[] = [];
  for (const value of content) {
    const block = record(value);
    const name = `Tool image ${images.length + 1}`;
    let image: ToolImage | null = null;
    if (block.type === "image") {
      const source = record(block.source);
      image = base64Image(store, source.type === "base64" ? source.data : block.data, name);
    } else if (block.type === "inputImage" || block.type === "input_image") {
      image = dataUrlImage(store, block.imageUrl ?? block.image_url, name);
    } else {
      const resource = imageResource(block);
      if (resource) image = base64Image(store, resource.blob, name);
    }
    if (image) images.push(image);
    if (images.length >= 8) break;
  }
  return images;
}
