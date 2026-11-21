// Builds the `condensate` CLI and places it where Tauri bundles it (bundle.externalBin), so the
// installer ships it next to the app. Usage: node scripts/sidecar.mjs [--release]
import { execSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const release = process.argv.includes("--release");
const host = execSync("rustc -vV").toString().match(/host: (\S+)/)[1];
const ext = process.platform === "win32" ? ".exe" : "";

execSync(`cargo build -p condensate-cli${release ? " --release" : ""}`, { cwd: repo, stdio: "inherit" });

const src = join(repo, "target", release ? "release" : "debug", `condensate${ext}`);
const destDir = join(here, "..", "src-tauri", "binaries");
mkdirSync(destDir, { recursive: true });
copyFileSync(src, join(destDir, `condensate-${host}${ext}`));
console.log(`sidecar: condensate-${host}${ext}`);
