/**
 * realsite_launch.mjs - launch options for the real-site board's Chrome oracle.
 *
 * Headless by default. Headed (Pete, A1, 2026-09-26) is opt-in: the
 * HeadlessChrome identity gets some oracle pages blocked (x "Access denied",
 * carvana Cloudflare), and Chrome identifying as itself is not evasion: no
 * webdriver change, no spoofing, no challenge solving.
 *
 * A headed window is a real window on the seat, so it is allowed only while
 * the runner holds the real-window lock (agent-briefs, 11-06; R1 HOLD on
 * #960). realsite_board.py --oracle-headed checks HID idle, takes
 * ~/.claude/realwindow.lock, then sets REALSITE_ORACLE_HEADED=1,
 * REALSITE_REALWINDOW_LOCK and REALSITE_LOCK_OWNER (its own pid, which it
 * wrote to the lock's pid file). Here, a headed request goes headed only if
 * that lock's pid file equals REALSITE_LOCK_OWNER: a lock directory that
 * exists but is someone else's (another runner, or the shell recipe's bare
 * mkdir) keeps the oracle headless, with the reason on stderr.
 * REALSITE_ORACLE_HEADLESS=1 (the old --oracle-headless) always wins.
 * Every deterministic flag is kept.
 */

import { readFileSync } from 'fs';
import { homedir } from 'os';
import { join } from 'path';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

/** Headed or headless, and why. Headed needs the request AND ownership of the lock. */
export function oracleHeadedDecision(env = process.env) {
  if (env.REALSITE_ORACLE_HEADED !== '1') return { headed: false, reason: 'headless (default)' };
  if (env.REALSITE_ORACLE_HEADLESS === '1') {
    return { headed: false, reason: 'headless: REALSITE_ORACLE_HEADLESS=1 wins' };
  }
  const lock = env.REALSITE_REALWINDOW_LOCK || join(homedir(), '.claude', 'realwindow.lock');
  const owner = (env.REALSITE_LOCK_OWNER || '').trim();
  let holder;
  try {
    holder = readFileSync(join(lock, 'pid'), 'utf8').trim();
  } catch (e) {
    return { headed: false, reason: `headless: real-window lock ${lock} has no holder pid (${e.code || e.message})` };
  }
  if (!/^[0-9]+$/.test(owner)) {
    return { headed: false, reason: `headless: headed asked without REALSITE_LOCK_OWNER; ${lock} is held by pid ${holder || '?'}` };
  }
  if (holder !== owner) {
    return { headed: false, reason: `headless: real-window lock ${lock} is held by pid ${holder || '?'}, not ${owner}` };
  }
  return { headed: true, reason: `headed: pid ${owner} holds ${lock}` };
}

export function oracleLaunchOptions(width, height, env = process.env) {
  const { headed, reason } = oracleHeadedDecision(env);
  if (!headed && env.REALSITE_ORACLE_HEADED === '1') console.error(`realsite oracle: ${reason}`);
  const launch = getDeterministicLaunchOptions();
  launch.headless = !headed;
  if (headed) {
    launch.args = [...(launch.args || []), '--window-position=-2400,0', `--window-size=${width},${height}`];
  }
  return launch;
}
