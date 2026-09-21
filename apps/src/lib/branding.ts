export const APP_DISPLAY_NAME = "团队中转站";

/** Apply team branding only to application-owned display templates. */
export function brandDisplayText(template: string): string {
  return template.replace(/\bCodexManager\b/g, APP_DISPLAY_NAME);
}
