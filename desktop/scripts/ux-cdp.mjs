#!/usr/bin/env node
import { main } from "../test-harness/ux-cdp/cli.mjs";

main().catch(err => {
  console.error(err.stack || String(err));
  process.exit(1);
});
