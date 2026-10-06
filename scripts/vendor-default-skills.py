"""Refresh the pinned, MIT-licensed instruction bundle. Never run upstream setup."""

import json
import pathlib
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEST = ROOT / "rust/crates/runtime/bundled-skills"
PROJECTS = {
    "ponytail": "DietrichGebert/ponytail",
    "antislop": "miqdadbadjuber/anti-slop",
    "gstack": "garrytan/gstack",
    "agent-reach": "Panniantong/Agent-Reach",
}
AGENT_REACH_ADAPTER = """## Neuron host adapter

Agent Reach's routing instructions are active on every conversation turn. Use
the appropriate documented route for internet research when its tools are
available. This is a skill and CLI toolkit, not an MCP server. Verify executable
availability before running doctor or platform commands; do not invent a stdio
launch command. The upstream author's conda `dl` environment is not a Neuron
requirement. Use discovered executables and the operating system's temporary
directory. The upstream workspace rule applies to temporary research downloads,
not source edits or artifacts requested by the user. Installing a channel,
accessing cookies or a browser session, sending
data to an external service, or writing to a platform requires authorization
under Neuron's tool policy. Missing dependencies must be reported honestly;
continue independent work using available tools without claiming missing checks
passed. User instructions take precedence over these workflow instructions.

"""
GSTACK_SKILLS = ("investigate", "review", "qa", "qa-only", "ship")
GSTACK_ADAPTER = """## Neuron host adapter

This instruction bundle ships without gstack's host executables. Do not execute
the upstream preamble, setup, telemetry, bookkeeping or external-helper commands.
Continue with investigation, review and evidence-based verification using Neuron
tools. Do not request installation just because those upstream helpers are absent.
Resolve `~/.claude/skills/gstack/` references relative to the extracted gstack root
shown in the system prompt. If a referenced file or tool is absent, report the
limitation and continue independent work; never count that check as passed.
User instructions and Neuron tool permissions take precedence over this workflow.

"""
GSTACK_ROUTER = """---
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
"""


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "Neuron-skill-vendor"})
    with urllib.request.urlopen(request, timeout=45) as response:
        return response.read().decode("utf-8")


def write(relative, text):
    target = DEST / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8", newline="\n")


def main():
    if "--manifest-only" in sys.argv:
        generate_manifest()
        return
    selected = next((arg.split("=", 1)[1] for arg in sys.argv if arg.startswith("--project=")), None)
    if selected and selected not in PROJECTS:
        raise ValueError(f"Unknown skill project: {selected}")
    provenance = json.loads((DEST / "SOURCES.json").read_text()) if selected else {}
    for name, repo in PROJECTS.items():
        if selected and name != selected:
            continue
        commit = json.loads(fetch(f"https://api.github.com/repos/{repo}/commits/HEAD"))["sha"]
        base = f"https://raw.githubusercontent.com/{repo}/{commit}/"
        provenance[name] = {"repository": f"https://github.com/{repo}", "commit": commit, "license": "MIT"}
        write(f"licenses/{name}.txt", fetch(base + "LICENSE"))
        if name == "ponytail":
            write("ponytail/SKILL.md", fetch(base + "skills/ponytail/SKILL.md"))
        elif name == "antislop":
            core = fetch(base + "antislop.md")
            start = core.index("## What This Is")
            write("antislop/SKILL.md", "---\nname: antislop\ndescription: Filter generic interfaces, prose and code comments.\n---\n# AntiSlop (Neuron adapter)\n\nDefault mode: during. Apply the following source rules where relevant. The bundle is already installed; do not run an install wizard or ask for a mode. User instructions and design direction take precedence.\n\n" + core[start:])
            for skill in ("antislop-ui", "antislop-copywriting", "antislop-code", "antislop-human", "antislop-layoutmobile"):
                write(f"{skill}/SKILL.md", fetch(base + f"skills/{skill}/SKILL.md"))
            write("antislop-human/contrast-check.py", fetch(base + "skills/antislop-human/contrast-check.py"))
        elif name == "agent-reach":
            tree = json.loads(fetch(f"https://api.github.com/repos/{repo}/git/trees/{commit}?recursive=1"))["tree"]
            prefix = "agent_reach/skill/"
            for entry in tree:
                path = entry["path"]
                if entry["type"] == "blob" and path.startswith(prefix) and path.endswith(".md"):
                    if path == prefix + "SKILL_en.md":
                        continue
                    contents = fetch(base + path)
                    if path == prefix + "SKILL.md":
                        contents = fetch(base + prefix + "SKILL_en.md")
                        boundary = contents.index("\n---\n") + len("\n---\n")
                        contents = contents[:boundary] + "\n" + AGENT_REACH_ADAPTER + contents[boundary:]
                    write(f"agent-reach/{path[len(prefix):]}", contents)
        else:
            tree = json.loads(fetch(f"https://api.github.com/repos/{repo}/git/trees/{commit}?recursive=1"))["tree"]
            for entry in tree:
                path = entry["path"]
                if entry["type"] == "blob" and path.split("/")[0] in GSTACK_SKILLS and (path.endswith(".md") or path.endswith(".json")) and not path.endswith(".tmpl"):
                    contents = fetch(base + path)
                    if path.endswith("/SKILL.md"):
                        skill = path.split("/")[0]
                        contents = contents.replace(f"name: {skill}\n", f"name: gstack-{skill}\n", 1)
                        boundary = contents.index("\n---\n") + len("\n---\n")
                        contents = contents[:boundary] + "\n" + GSTACK_ADAPTER + contents[boundary:]
                    write(f"gstack/{path}", contents)
            write("gstack/BROWSER.md", fetch(base + "BROWSER.md"))
            write("gstack/ETHOS.md", fetch(base + "ETHOS.md"))
    write("SOURCES.json", json.dumps(provenance, indent=2) + "\n")
    generate_manifest()
    print(json.dumps(provenance, indent=2))


def generate_manifest():
    write("gstack/SKILL.md", GSTACK_ROUTER)
    entries = []
    for path in sorted(DEST.rglob("*")):
        if path.is_file():
            relative = path.relative_to(DEST).as_posix()
            entries.append(f'    ("{relative}", include_str!("../bundled-skills/{relative}")),')
    generated = "// Generated by scripts/vendor-default-skills.py.\n" + "pub const FILES: &[(&str, &str)] = &[\n" + "\n".join(entries) + "\n];\n"
    (ROOT / "rust/crates/runtime/src/default_skill_files.rs").write_text(generated, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
