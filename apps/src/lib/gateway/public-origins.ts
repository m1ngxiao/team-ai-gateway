/** Public addresses are embedded in the static UI at build time, never secrets. */
export type PublicOriginEnvironment = {
  NEXT_PUBLIC_API_ORIGIN?: string;
  NEXT_PUBLIC_ADMIN_ORIGIN?: string;
  NEXT_PUBLIC_STATS_ORIGIN?: string;
};

const LOCAL_ORIGINS = {
  NEXT_PUBLIC_API_ORIGIN: "http://127.0.0.1:48760",
  NEXT_PUBLIC_ADMIN_ORIGIN: "http://127.0.0.1:48761",
  NEXT_PUBLIC_STATS_ORIGIN: "http://127.0.0.1:48763",
} as const;

export function validatePublicOrigin(value: string, name: string): string {
  const normalized = value.trim();
  const invalid = () => new Error(
    `${name} must be an HTTPS origin (or HTTP on loopback), without credentials, a path, query, or fragment.`,
  );

  // Inspect the input before URL normalization can discard empty queries,
  // credentials, or dot segments. A single trailing root slash is harmless.
  if (!/^https?:\/\/[^/?#@\\\s]+\/?$/i.test(normalized)) throw invalid();

  let url: URL;
  try {
    url = new URL(normalized);
  } catch {
    throw invalid();
  }

  const loopback = url.hostname === "localhost"
    || url.hostname === "[::1]"
    || /^127(?:\.\d{1,3}){3}$/.test(url.hostname);
  if (
    (url.protocol !== "https:" && !(url.protocol === "http:" && loopback))
    || url.username || url.password || url.pathname !== "/" || url.search || url.hash
  ) throw invalid();

  return url.origin;
}

export function resolvePublicOrigins(environment: PublicOriginEnvironment) {
  const read = (key: keyof PublicOriginEnvironment) =>
    validatePublicOrigin(environment[key] ?? LOCAL_ORIGINS[key], key);
  return Object.freeze({
    api: read("NEXT_PUBLIC_API_ORIGIN"),
    admin: read("NEXT_PUBLIC_ADMIN_ORIGIN"),
    stats: read("NEXT_PUBLIC_STATS_ORIGIN"),
  });
}

// Keep direct property access: Next.js replaces NEXT_PUBLIC_* at build time.
export const PUBLIC_ORIGINS = resolvePublicOrigins({
  NEXT_PUBLIC_API_ORIGIN: process.env.NEXT_PUBLIC_API_ORIGIN,
  NEXT_PUBLIC_ADMIN_ORIGIN: process.env.NEXT_PUBLIC_ADMIN_ORIGIN,
  NEXT_PUBLIC_STATS_ORIGIN: process.env.NEXT_PUBLIC_STATS_ORIGIN,
});
