#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

const ROOT = process.cwd();
const TARGET_DIRS = ["src", "src-tauri/src"];
const ALLOWED_EXTENSIONS = new Set([".ts", ".tsx", ".rs"]);

const BANNED_EVENT_NAMES = [
  "notifications-pause-state",
  "notifications-pause-mode",
  "notifications-paused-until",
  "notifications-resumed",
  "messages-synced",
  "messages-updated",
  "message-received",
  "connection-state",
  "runtime-diagnostics",
  "connection-error",
];

const DISALLOWED_PATTERNS = [
  {
    id: "backend-app-emit",
    regex: /\bapp\.emit\(/g,
    message: "Use app update channels or targeted emit_to (app.emit is disallowed).",
  },
  {
    id: "backend-window-emit",
    regex: /\bwindow\.emit\(/g,
    message: "Use app update channels or targeted emit_to (window.emit is disallowed).",
  },
];

function collectFiles(dir) {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectFiles(full));
      continue;
    }
    if (!ALLOWED_EXTENSIONS.has(path.extname(entry.name))) {
      continue;
    }
    files.push(full);
  }
  return files;
}

function findLineNumber(content, index) {
  let line = 1;
  for (let i = 0; i < index; i += 1) {
    if (content[i] === "\n") {
      line += 1;
    }
  }
  return line;
}

const findings = [];
for (const targetDir of TARGET_DIRS) {
  const absoluteDir = path.join(ROOT, targetDir);
  if (!fs.existsSync(absoluteDir)) continue;
  const files = collectFiles(absoluteDir);
  for (const file of files) {
    const content = fs.readFileSync(file, "utf8");

    for (const eventName of BANNED_EVENT_NAMES) {
      let idx = content.indexOf(eventName);
      while (idx !== -1) {
        findings.push({
          file: path.relative(ROOT, file),
          line: findLineNumber(content, idx),
          kind: "banned-event-name",
          detail: `Found legacy event name "${eventName}"`,
        });
        idx = content.indexOf(eventName, idx + eventName.length);
      }
    }

    if (file.endsWith(".rs")) {
      for (const pattern of DISALLOWED_PATTERNS) {
        const matches = content.matchAll(pattern.regex);
        for (const match of matches) {
          findings.push({
            file: path.relative(ROOT, file),
            line: findLineNumber(content, match.index ?? 0),
            kind: pattern.id,
            detail: pattern.message,
          });
        }
      }
    }
  }
}

if (findings.length > 0) {
  console.error("Interaction standard violations detected:\n");
  for (const finding of findings) {
    console.error(
      `- ${finding.file}:${finding.line} [${finding.kind}] ${finding.detail}`
    );
  }
  process.exit(1);
}

console.log("Interaction standard check passed.");
