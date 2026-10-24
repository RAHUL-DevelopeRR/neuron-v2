---
name: gstack
description: Route Neuron tasks to the bundled gstack workflows.
---
# Gstack routing (Neuron adapter)

Load the applicable workflow with the Skill tool before doing its work:
- Debugging: gstack-investigate.
- Code review: gstack-review.
- Fixing and verifying an authorized change: gstack-qa.
- Reporting QA findings without changes: gstack-qa-only.
- Preparing an authorized release: gstack-ship.

Follow its investigation, review and verification gates. User instructions take
precedence. Adapt host-specific tool names to Neuron's discovered tools. Do not
run upstream onboarding, telemetry, external-helper scripts or Conductor/GBrain
bookkeeping when those dependencies are absent. Missing tools remain unavailable;
do not claim browser checks, tests or releases passed without evidence.

Companion files are relative to this directory. Browser automation requires an
explicitly configured MCP server; these instructions install no browser runtime.
