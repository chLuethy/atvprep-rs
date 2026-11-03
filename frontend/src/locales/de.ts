import { deActivity } from "./de-activity";
import { deCommon } from "./de-common";
import { deFiles, deFixPlan, deLibraryModels } from "./de-files";
import { deColumns, deLibrary, deMenusAndKeys } from "./de-library";
import { deInspector } from "./de-inspector";
import { dePreview } from "./de-preview";
import { dePages, deShell } from "./de-pages";
import { deSchedules } from "./de-schedules";

/**
 * The German texts. The key is the English text exactly as it is written in the code; a value with
 * two parts is [singular, plural] for `tn`. Technical output (verify findings, server messages, job
 * logs) is not translated: it comes from the tools and stays as they wrote it.
 */
export const de: Record<string, string | readonly [string, string]> = {
  ...deCommon,
  ...deFiles,
  ...deFixPlan,
  ...deLibraryModels,
  ...deActivity,
  ...deSchedules,
  ...deLibrary,
  ...deColumns,
  ...deMenusAndKeys,
  ...deInspector,
  ...dePreview,
  ...dePages,
  ...deShell,
};
