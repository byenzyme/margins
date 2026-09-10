#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const here = path.dirname(new URL(import.meta.url).pathname);
const desktop = path.resolve(here, '../..');
const repo = path.resolve(desktop, '..');
const tauri = path.join(desktop, 'src-tauri');
const fixtureDir = path.join(here, 'fixtures', 'acme-pilot-review');
const fixture = JSON.parse(fs.readFileSync(path.join(fixtureDir, 'settings.json'), 'utf8'));

const stateRoot = process.env.MARGINS_E2E_STATE ?? '/tmp/margins-e2e';
const home = process.env.HOME ?? path.join(stateRoot, 'home');
const dataDir = process.env.MARGINS_DATA_DIR ?? path.join(stateRoot, 'data');
const baseUrl = process.env.MARGINS_E2E_SERVER ?? 'http://127.0.0.1:8787';
const tokenPath = process.env.MARGINS_TOKEN_FILE ?? path.join(dataDir, 'token');
const token = fs.readFileSync(tokenPath, 'utf8').trim();
const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target-linux-server');
const stageBin = path.join(targetDir, 'debug', 'examples', 'margins-e2e-stage');

function expandTilde(value) {
  if (value === '~') return home;
  if (value.startsWith('~/')) return path.join(home, value.slice(2));
  return value;
}

function activeProject(settings) {
  const id = settings.active_project_id;
  return settings.projects?.find((project) => project.id === id) ?? settings.projects?.[0] ?? null;
}

function resolveWorkDir(settings) {
  const project = activeProject(settings);
  const vault = project?.path ?? settings.vault_path;
  return vault?.trim() ? expandTilde(vault) : path.join(home, 'margins-sessions');
}

async function invoke(command, body = {}) {
  const response = await fetch(`${baseUrl}/api/invoke/${command}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    throw new Error(`${command} returned non-JSON HTTP ${response.status}: ${text}`);
  }
  if (!response.ok || json.ok === false) {
    throw new Error(`${command} failed: ${json.error ?? text}`);
  }
  return json.result;
}

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    stdio: 'pipe',
    encoding: 'utf8',
    ...options,
    env: {
      ...process.env,
      HOME: home,
      MARGINS_PROFILE: process.env.MARGINS_PROFILE ?? 'e2e',
      MARGINS_DISABLE_KEYCHAIN: process.env.MARGINS_DISABLE_KEYCHAIN ?? '1',
      CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target-linux-server'),
      ...(options.env ?? {}),
    },
  });
  if (result.status !== 0) {
    throw new Error(
      `${cmd} ${args.join(' ')} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  }
  return result.stdout.trim();
}

async function main() {
  const currentSettings = await invoke('get_settings');
  const workDir = resolveWorkDir(currentSettings);
  console.log('fixture_settings=not_applied (using running server settings)');

  const sessionNameOverride = process.env.MARGINS_E2E_SESSION_NAME?.trim();
  const aligned = fs.readFileSync(path.join(fixtureDir, 'aligned.md'), 'utf8');
  const memo = fs.readFileSync(path.join(fixtureDir, 'memo.md'), 'utf8');
  let staged;
  try {
    staged = await invoke('import_transcript', {
      name: sessionNameOverride || fixture.session_name,
      title: fixture.event_title,
      people: fixture.people,
      transcript: aligned,
      memo,
    });
    console.log(`staged_via=import_transcript`);
  } catch (error) {
    console.warn(`import_transcript_failed=${error.message}`);
    console.warn('staged_via=margins-e2e-stage');
    const stageOut = run(
      fs.existsSync(stageBin) ? stageBin : 'cargo',
      fs.existsSync(stageBin) ? [
        '--work-dir',
        workDir,
        '--fixture-dir',
        fixtureDir,
        '--name',
        sessionNameOverride || fixture.session_name,
        '--event-title',
        fixture.event_title,
        '--people-json',
        JSON.stringify(fixture.people),
      ] : [
        'run',
        '--quiet',
        '--no-default-features',
        '--features',
        'server',
        '--example',
        'margins-e2e-stage',
        '--',
        '--work-dir',
        workDir,
        '--fixture-dir',
        fixtureDir,
        '--name',
        fixture.session_name,
        '--event-title',
        fixture.event_title,
        '--people-json',
        JSON.stringify(fixture.people),
      ],
      { cwd: tauri },
    );
    staged = JSON.parse(stageOut);
  }
  console.log(`staged=${JSON.stringify(staged, null, 2)}`);
  const sessionName = staged.name ?? staged.session ?? sessionNameOverride ?? fixture.session_name;
  if (process.env.MARGINS_E2E_STAGE_ONLY === '1') {
    console.log(`session_name=${sessionName}`);
    console.log('stage_only=true');
    return;
  }

  const processStarted = Date.now();
  await invoke('process_session', {
    name: sessionName,
    forceTranscribe: false,
  });

  const trace = await invoke('get_distill_trace', { name: sessionName });
  const sessions = await invoke('list_sessions', {});
  const session = sessions.find((entry) => entry.name === sessionName);
  let finalVaultPath = session?.vault_note_path ?? null;
  if (!finalVaultPath) {
    const inbox = path.join(workDir, fixture.inbox_folder);
    const candidates = fs
      .readdirSync(inbox, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith('.md'))
      .map((entry) => path.join(inbox, entry.name))
      .filter((file) => fs.statSync(file).mtimeMs >= processStarted - 1000)
      .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
    finalVaultPath = candidates[0] ?? null;
  }
  const noteMarkdown = finalVaultPath && fs.existsSync(finalVaultPath)
    ? fs.readFileSync(finalVaultPath, 'utf8')
    : '';

  console.log(`note_generated=${Boolean(noteMarkdown)}`);
  console.log(`final_vault_path=${finalVaultPath ?? ''}`);
  console.log(`trace_events=${trace.length}`);
  console.log('--- note markdown ---');
  console.log(noteMarkdown);
}

main().catch((error) => {
  console.error(error?.stack ?? String(error));
  process.exit(1);
});
