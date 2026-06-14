#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const https = require("https");
const { execFileSync } = require("child_process");

const pkg = require(path.join(__dirname, "..", "package.json"));
const REPO = "Midwess/PgPaw";

const TARGETS = {
  "linux:x64": "x86_64-unknown-linux-gnu",
  "darwin:x64": "x86_64-apple-darwin",
  "darwin:arm64": "aarch64-apple-darwin",
};

const platformKey = `${process.platform}:${process.arch}`;
const target = TARGETS[platformKey];

if (!target) {
  console.error(
    `[pgpaw] no prebuilt binary for ${platformKey}. ` +
      `Install from source instead: cargo install pgpaw`
  );
  process.exit(0);
}

const asset = `pgpaw-${target}.tar.gz`;
const url = `https://github.com/${REPO}/releases/download/v${pkg.version}/${asset}`;
const binDir = path.join(__dirname, "..", "bin");
const tarPath = path.join(binDir, asset);

function download(fileUrl, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    https
      .get(fileUrl, (res) => {
        if (
          [301, 302, 303, 307, 308].includes(res.statusCode) &&
          res.headers.location
        ) {
          res.resume();
          if (redirects > 10) return reject(new Error("too many redirects"));
          return resolve(download(res.headers.location, dest, redirects + 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${fileUrl}`));
        }
        const file = fs.createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => file.close(resolve));
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

(async () => {
  try {
    fs.mkdirSync(binDir, { recursive: true });
    console.log(`[pgpaw] downloading ${url}`);
    await download(url, tarPath);
    execFileSync("tar", ["-xzf", tarPath, "-C", binDir]);
    fs.unlinkSync(tarPath);
    fs.chmodSync(path.join(binDir, "pgpaw"), 0o755);
    console.log("[pgpaw] native binary installed");
  } catch (err) {
    console.error(`[pgpaw] postinstall failed: ${err.message}`);
    process.exit(0);
  }
})();
