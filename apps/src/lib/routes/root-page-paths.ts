export const ROOT_PAGE_PATHS = [
  "/",
  "/accounts",
  "/claude-accounts",
  "/account-manager",
  "/aggregate-api",
  "/apikeys",
  "/quota",
  "/projects",
  "/models",
  "/model-groups",
  "/plugins",
  "/skills",
  "/logs",
  "/settings",
] as const;

export type RootPagePath = (typeof ROOT_PAGE_PATHS)[number];
