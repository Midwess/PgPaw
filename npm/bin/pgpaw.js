#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const bin = path.join(
  __dirname,
  process.platform === "win32" ? "pgpaw.exe" : "pgpaw"
);

if (!fs.existsSync(bin)) {
  console.error(
    "[pgpaw] native binary missing. Reinstall the package, or check " +
      "platform support at https://github.com/Midwess/PgPaw/releases"
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
process.exit(result.status === null ? 1 : result.status);
