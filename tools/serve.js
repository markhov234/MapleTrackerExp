// Minimal static file server for the UI during development.
// No dependencies, serves ui/ only, refuses paths that escape it.
const http = require("http");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..", "ui");
const PORT = Number(process.env.PORT) || 5173;

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2"
};

http
  .createServer((req, res) => {
    let rel = decodeURIComponent(req.url.split("?")[0]);
    if (rel === "/" || rel === "") rel = "/index.html";

    const full = path.resolve(ROOT, "." + rel);
    if (full !== ROOT && !full.startsWith(ROOT + path.sep)) {
      res.writeHead(403).end("forbidden");
      return;
    }

    fs.readFile(full, (err, buf) => {
      if (err) {
        res.writeHead(404, { "content-type": "text/plain" }).end("not found: " + rel);
        return;
      }
      res.writeHead(200, {
        "content-type": TYPES[path.extname(full).toLowerCase()] || "application/octet-stream",
        "cache-control": "no-store"
      });
      res.end(buf);
    });
  })
  .listen(PORT, "127.0.0.1", () => {
    console.log("EXP Calc UI on http://127.0.0.1:" + PORT + "  (serving " + ROOT + ")");
  });
