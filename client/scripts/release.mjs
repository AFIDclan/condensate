// Build a signed release of the desktop app and publish it to the Condensate server:
//   - Windows: the installer, under its versioned name and as Condensate-setup.exe (the link to share)
//   - Linux: an AppImage (updates itself) and a .deb, built on the server over ssh
//   - latest.json, which installed apps poll to update themselves
//   - index.html, the download page at <server>/download/
//
// Usage: node scripts/release.mjs ["release notes"] [--only windows|linux|page]
// Windows builds on this machine. Linux builds in ~/condensate on the deploy host, which needs
// Rust, Node in ~/.local/node and the Tauri Linux packages (see README). The signing key at
// ~/.tauri/condensate.key is streamed to that build over ssh and never written to disk there.
// `--only page` rebuilds just the download page from the published latest.json.
// Env: CONDENSATE_DEPLOY_HOST (default oort-tunnel), CONDENSATE_SERVER (public URL).
import { execSync, spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const client = join(here, "..");
const repo = join(client, "..");
const host = process.env.CONDENSATE_DEPLOY_HOST ?? "oort-tunnel";
const conf = JSON.parse(readFileSync(join(client, "src-tauri", "tauri.conf.json"), "utf8"));
const server = (process.env.CONDENSATE_SERVER ?? conf.plugins.updater.endpoints[0].replace(/\/download\/latest\.json$/, "")).replace(/\/$/, "");

const args = process.argv.slice(2);
const onlyAt = args.indexOf("--only");
const only = onlyAt >= 0 ? args.splice(onlyAt, 2)[1] : null;
if (only && !["windows", "linux", "page"].includes(only)) throw new Error("--only takes windows, linux or page");
const buildWindows = !only || only === "windows";
const buildLinux = !only || only === "linux";

const out = mkdtempSync(join(tmpdir(), "condensate-release-"));
const download = (file) => `${server}/download/${file}`;
const published = only ? await fetch(download("latest.json")).then((res) => (res.ok ? res.json() : null)).catch(() => null) : null;

let version = conf.version;
let platforms = {};
const uploads = []; // files in `out` to copy to the server, before index.html and latest.json

if (only === "page") {
  if (!published) throw new Error(`no latest.json at ${download("latest.json")} to build the page from`);
  version = published.version;
  platforms = published.platforms ?? {};
} else {
  const keyPath = join(homedir(), ".tauri", "condensate.key");
  if (!existsSync(keyPath)) throw new Error(`missing signing key ${keyPath}`);
  const key = readFileSync(keyPath, "utf8");

  if (buildWindows) {
    if (process.platform !== "win32") throw new Error("the Windows build has to run on Windows (or pass --only linux)");
    console.log(`Building Condensate ${version} for Windows...`);
    execSync("npm run tauri build", {
      cwd: client,
      stdio: "inherit",
      env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: key, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "" },
    });
    const bundle = join(repo, "target", "release", "bundle", "nsis");
    const installer = `Condensate_${version}_x64-setup.exe`;
    copyFileSync(join(bundle, installer), join(out, installer));
    copyFileSync(join(bundle, installer), join(out, "Condensate-setup.exe"));
    uploads.push(installer, "Condensate-setup.exe");
    platforms["windows-x86_64"] = { signature: readFileSync(join(bundle, `${installer}.sig`), "utf8").trim(), url: download(installer) };
  }

  if (buildLinux) {
    await syncSource();
    console.log(`Building Condensate ${version} for Linux on ${host}...`);
    ssh(
      [
        "set -e",
        'export PATH="$HOME/.local/node/bin:$HOME/.cargo/bin:$PATH"',
        "cd ~/condensate/client",
        "npm ci --no-audit --no-fund --loglevel=error",
        'export TAURI_SIGNING_PRIVATE_KEY="$(cat)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""',
        "npx tauri build",
      ],
      { input: key, stdio: ["pipe", "inherit", "inherit"] },
    );
    // Copy the bundles into the public downloads folder and report their signatures.
    const report = ssh(
      [
        "set -e",
        "B=~/condensate/target/release/bundle",
        "D=~/condensate-server/data/downloads",
        `A=$(ls "$B"/appimage/*_${version}_*.AppImage)`,
        `DEB=$(ls "$B"/deb/*_${version}_*.deb)`,
        'cp "$A" "$DEB" "$D/"',
        'cp "$A" "$D/Condensate-x86_64.AppImage"',
        'cp "$DEB" "$D/Condensate-amd64.deb"',
        'echo "appimage=$(basename "$A")"',
        'echo "appimage_sig=$(cat "$A.sig")"',
        'echo "deb=$(basename "$DEB")"',
        'if [ -f "$DEB.sig" ]; then echo "deb_sig=$(cat "$DEB.sig")"; fi',
      ],
      { stdio: ["ignore", "pipe", "inherit"] },
    );
    const r = Object.fromEntries(report.split("\n").filter((l) => l.includes("=")).map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1).trim()]));
    const appimage = { signature: r.appimage_sig, url: download(r.appimage) };
    platforms["linux-x86_64"] = appimage;
    platforms["linux-x86_64-appimage"] = appimage;
    // Installs from the .deb update themselves too, when the updater signed it.
    if (r.deb_sig) platforms["linux-x86_64-deb"] = { signature: r.deb_sig, url: download(r.deb) };
  }

  if (only) {
    // Keep the other platform's entries if they are for this same version.
    if (published?.version === version) {
      for (const [k, v] of Object.entries(published.platforms ?? {})) platforms[k] ??= v;
    } else if (published) {
      console.warn(`latest.json was for ${published.version}; only ${only} will be offered ${version}.`);
    }
  }

  writeFileSync(join(out, "latest.json"), JSON.stringify({ version, notes: args[0] ?? `Condensate ${version}`, pub_date: new Date().toISOString(), platforms }, null, 2));
}

writeFileSync(join(out, "index.html"), downloadPage(version, "windows-x86_64" in platforms, "linux-x86_64" in platforms));

console.log(`Uploading to ${host}...`);
// latest.json goes last, so apps only see the update once the installers are in place.
const files = [...uploads, "index.html", ...(only === "page" ? [] : ["latest.json"])].map((f) => `"${join(out, f)}"`).join(" ");
execSync(`scp -q ${files} ${host}:condensate-server/data/downloads/`, { stdio: "inherit" });
console.log(only === "page" ? `Updated the download page: ${server}/download/` : `Published ${version} (${Object.keys(platforms).join(", ")}): ${server}/download/`);

/** The download page: one button for the visitor's OS, the other downloads as small links, install tips folded away. */
function downloadPage(version, hasWindows, hasLinux) {
  const options = [
    hasWindows && { id: "windows", label: "Windows", button: "Download for Windows", href: "Condensate-setup.exe" },
    hasLinux && { id: "linux", label: "Linux AppImage", button: "Download for Linux", href: "Condensate-x86_64.AppImage" },
    hasLinux && { id: "deb", label: ".deb", button: "Download .deb", href: "Condensate-amd64.deb" },
  ].filter(Boolean);
  const help = [
    hasWindows && `<p><b>Windows</b> If SmartScreen warns you, click <i>More info</i>, then <i>Run anyway</i>.</p>`,
    hasLinux && `<p><b>AppImage</b> Run <code>chmod +x Condensate-x86_64.AppImage</code>, then open it. Ubuntu 24.04+ also needs <code>sudo apt install libfuse2t64</code>.</p>`,
    hasLinux && `<p><b>.deb</b> <code>sudo apt install ./Condensate-amd64.deb</code></p>`,
  ].filter(Boolean);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Condensate</title>
<link rel="icon" href="data:image/svg+xml,${encodeURIComponent(readFileSync(join(client, "public", "icon.svg"), "utf8"))}">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Bebas+Neue&family=Inter:wght@400;600&display=swap" rel="stylesheet">
<style>
*{box-sizing:border-box}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:#08080a;color:#f4f4f6;font:15px/1.5 Inter,system-ui,sans-serif;-webkit-font-smoothing:antialiased}
main{width:100%;max-width:400px;padding:48px 16px;text-align:center}
.mark svg{display:block;width:72px;height:72px;margin:0 auto}
h1{font:400 60px/1 "Bebas Neue",Impact,sans-serif;letter-spacing:.05em;margin:20px 0 6px}
.tag{margin:0;color:rgba(235,235,240,.5)}
.btn{display:block;margin:36px 0 14px;padding:14px;border-radius:10px;background:#f4f4f6;color:#08080a;font-weight:600;text-decoration:none;transition:background .15s}
.btn:hover{background:#fff}
.more{margin:0;font-size:13px;color:rgba(235,235,240,.35)}
.more a{color:rgba(235,235,240,.6);text-decoration:none}
.more a:hover{color:#f4f4f6}
.sep{margin:0 8px}
details{margin-top:48px;font-size:13px;color:rgba(235,235,240,.55);text-align:left}
summary{cursor:pointer;list-style:none;text-align:center;color:rgba(235,235,240,.35)}
summary::-webkit-details-marker{display:none}
summary:hover{color:rgba(235,235,240,.7)}
details p{margin:12px 0 0}
details b{display:block;font-weight:600;color:#f4f4f6}
code{font:12px ui-monospace,Consolas,monospace;color:#f4f4f6;background:rgba(255,255,255,.06);padding:1px 5px;border-radius:4px;word-break:break-all}
</style>
</head>
<body>
<main>
<div class="mark">${readFileSync(join(client, "public", "icon.svg"), "utf8")}</div>
<h1>Condensate</h1>
<p class="tag">Games made by your friends.</p>
<a class="btn" id="primary" href="${options[0]?.href ?? "#"}">${options[0]?.button ?? "No downloads yet"}</a>
<p class="more" id="more"></p>
${help.length ? `<details><summary>Install help</summary>${help.join("")}</details>` : ""}
</main>
<script>
const options = ${JSON.stringify(options)};
const ua = navigator.userAgent;
const pick = (/Linux/.test(ua) && !/Android/.test(ua) && options.find((o) => o.id === "linux")) || options[0];
if (pick) {
  const primary = document.getElementById("primary");
  primary.href = pick.href;
  primary.textContent = pick.button;
}
document.getElementById("more").innerHTML = ["v${version}", ...options.filter((o) => o !== pick).map((o) => '<a href="' + o.href + '">' + o.label + "</a>")].join('<span class="sep">·</span>');
</script>
</body>
</html>
`;
}

/** Run a shell script (one command per array entry) on the deploy host; returns its stdout. */
function ssh(lines, opts) {
  const res = spawnSync("ssh", [host, lines.join("; ")], { ...opts, encoding: "utf8" });
  if (res.status !== 0) throw new Error(`remote command failed (exit ${res.status}): ${lines.at(-1)}`);
  return res.stdout ?? "";
}

/** Copy the source tree (no build output) to ~/condensate on the deploy host. */
function syncSource() {
  console.log(`Syncing source to ${host}...`);
  const excludes = ["target", "node_modules", ".git", "dist", "binaries", "data"].flatMap((e) => ["--exclude", e]);
  return new Promise((resolve, reject) => {
    const tar = spawn("tar", [...excludes, "-czf", "-", "."], { cwd: repo, stdio: ["ignore", "pipe", "inherit"] });
    const remote = spawn("ssh", [host, "mkdir -p ~/condensate && tar -xzf - -C ~/condensate"], { stdio: ["pipe", "inherit", "inherit"] });
    tar.stdout.pipe(remote.stdin);
    tar.on("error", reject);
    remote.on("error", reject);
    remote.on("close", (code) => (code === 0 ? resolve() : reject(new Error(`source sync failed (exit ${code})`))));
  });
}
