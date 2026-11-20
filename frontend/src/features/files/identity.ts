import type { FileRow } from "./loaders";

/** The server enforces (root_id, relative_path), not the display path across libraries. */
export const fileKey = (file: FileRow): string | undefined =>
  file.root_id && file.display_path ? JSON.stringify([file.root_id, file.display_path]) : undefined;
