// @lat: [[architecture#Project website]]
//
// Serves the static site in ../site and turns away AI training crawlers.
// robots.txt asks them to stay out; this enforces it for the ones that
// identify themselves honestly. Cloudflare's "Block AI bots" and Bot Fight
// Mode (dashboard settings) cover crawlers that lie about their user agent.

// User-agent substrings of crawlers that collect content for model training
// or bulk datasets. AI search and user-triggered fetchers (OAI-SearchBot,
// ChatGPT-User, Claude-User, PerplexityBot, ...) are not listed: they cite
// and link back. Keep this list in step with site/robots.txt.
const BLOCKED_AGENTS = [
  "gptbot",
  "claudebot",
  "anthropic-ai",
  "claude-web",
  "ccbot",
  "bytespider",
  "meta-externalagent",
  "facebookbot",
  "amazonbot",
  "cohere-ai",
  "cohere-training-data-crawler",
  "diffbot",
  "imagesiftbot",
  "omgili",
  "timpibot",
  "ai2bot",
  "pangubot",
  "kangaroo bot",
  "img2dataset",
  "velenpublicwebcrawler",
];

const SECURITY_HEADERS = {
  "Content-Security-Policy":
    "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; " +
    "img-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
  "Strict-Transport-Security": "max-age=31536000; includeSubDomains",
  "X-Content-Type-Options": "nosniff",
  "X-Frame-Options": "DENY",
  "Referrer-Policy": "strict-origin-when-cross-origin",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=(), interest-cohort=()",
  "Cross-Origin-Opener-Policy": "same-origin",
};

export function isBlockedAgent(userAgent) {
  const ua = (userAgent || "").toLowerCase();
  return BLOCKED_AGENTS.some((token) => ua.includes(token));
}

function withHeaders(response, extra = {}) {
  const res = new Response(response.body, response);
  for (const [name, value] of Object.entries({ ...SECURITY_HEADERS, ...extra })) {
    res.headers.set(name, value);
  }
  return res;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.hostname === "www.oxilitedb.com") {
      url.hostname = "oxilitedb.com";
      return Response.redirect(url.toString(), 301);
    }

    // robots.txt stays readable for everyone, so blocked crawlers can see why.
    if (url.pathname !== "/robots.txt" && isBlockedAgent(request.headers.get("user-agent"))) {
      return withHeaders(new Response("AI crawling is not permitted on this site. See /robots.txt.\n", { status: 403 }), {
        "Content-Type": "text/plain; charset=utf-8",
        "X-Robots-Tag": "noai, noimageai",
      });
    }

    return withHeaders(await env.ASSETS.fetch(request), { "X-Robots-Tag": "noai, noimageai" });
  },
};
