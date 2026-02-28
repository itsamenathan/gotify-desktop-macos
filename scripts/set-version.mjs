import fs from "node:fs";
import path from "node:path";

const raw = process.argv[2];
if (!raw) {
  console.error("Usage: npm run version:set -- <version>");
  process.exit(1);
}

const version = raw.trim().replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Invalid version '${raw}'. Expected semver like 0.2.1 or 0.2.1-rc.1`);
  process.exit(1);
}

const repoRoot = process.cwd();

const packageJsonPath = path.join(repoRoot, "package.json");
const tauriConfPath = path.join(repoRoot, "src-tauri", "tauri.conf.json");
const cargoTomlPath = path.join(repoRoot, "src-tauri", "Cargo.toml");

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
packageJson.version = version;
fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);

const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, "utf8"));
tauriConf.version = version;
fs.writeFileSync(tauriConfPath, `${JSON.stringify(tauriConf, null, 2)}\n`);

const cargoToml = fs.readFileSync(cargoTomlPath, "utf8");
const cargoVersionPattern = /(^\[package\][\s\S]*?^version\s*=\s*")([^"]+)("\s*$)/m;
const cargoMatch = cargoToml.match(cargoVersionPattern);
if (!cargoMatch) {
  console.error("Failed to find package version in src-tauri/Cargo.toml");
  process.exit(1);
}

const currentCargoVersion = cargoMatch[2];
const nextCargoToml = cargoToml.replace(
  cargoVersionPattern,
  `$1${version}$3`
);

if (nextCargoToml === cargoToml && currentCargoVersion !== version) {
  console.error("Failed to update version in src-tauri/Cargo.toml");
  process.exit(1);
}

fs.writeFileSync(cargoTomlPath, nextCargoToml);

console.log(`Set version to ${version} in package.json, src-tauri/tauri.conf.json, and src-tauri/Cargo.toml`);
