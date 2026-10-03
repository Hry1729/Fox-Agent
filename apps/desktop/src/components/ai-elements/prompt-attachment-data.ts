import type { FileUIPart } from "ai";
import type { AttachmentItem } from "./prompt-input";

/** Read an already-selected file. Rejects instead of resolving a placeholder. */
const readBlobAsDataUrl = (blob: Blob): Promise<string> =>
  // FileReader uses callback-based API, wrapping in Promise is necessary
  // oxlint-disable-next-line eslint-plugin-promise(avoid-new)
  new Promise((resolve, reject) => {
    const reader = new FileReader();
    // oxlint-disable-next-line eslint-plugin-unicorn(prefer-add-event-listener)
    reader.onload = () => {
      const result = reader.result;
      if (typeof result === "string" && result.startsWith("data:")) {
        resolve(result);
      } else {
        reject(new Error("attachment_read_failed"));
      }
    };
    // oxlint-disable-next-line eslint-plugin-unicorn(prefer-add-event-listener)
    reader.onerror = () => reject(new Error("attachment_read_failed"));
    // oxlint-disable-next-line eslint-plugin-unicorn(prefer-add-event-listener)
    reader.onabort = () => reject(new Error("attachment_read_failed"));
    reader.readAsDataURL(blob);
  });

/**
 * Fallback for an item that carries only a preview URL (a restored draft, or an
 * attachment added before this fix). It may fail under the app's CSP, and when it
 * does the caller must report the failure — never forward the blob URL.
 */
const readPreviewUrlAsDataUrl = async (url: string): Promise<string> => {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error("attachment_read_failed");
  }
  return readBlobAsDataUrl(await response.blob());
};

/** Resolve one attachment to the data: URL the conversation layer requires. */
const attachmentDataUrl = async (item: AttachmentItem): Promise<string> => {
  if (item.url?.startsWith("data:")) {
    return item.url;
  }
  if (item.file) {
    return readBlobAsDataUrl(item.file);
  }
  if (item.url?.startsWith("blob:")) {
    return readPreviewUrlAsDataUrl(item.url);
  }
  throw new Error("attachment_read_failed");
};

export const resolvePromptAttachments = (files: AttachmentItem[]): Promise<FileUIPart[]> =>
  Promise.all(files.map(async ({ id, file, ...item }) => ({
    ...item,
    url: await attachmentDataUrl({ ...item, id, file }),
  })));
