import type { FileRow } from "./loaders";

/** Der Server erzwingt (root_id, relative_path), nicht mediathekübergreifend den Anzeigepfad. */
export const fileKey = (file: FileRow): string | undefined =>
  file.root_id && file.display_path ? JSON.stringify([file.root_id, file.display_path]) : undefined;
