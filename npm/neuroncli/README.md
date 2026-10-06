# @zero-x.live/neuron

Install the Neuron terminal coding agent with Node.js 22.14 or later:

```sh
npm install --global @zero-x.live/neuron
neuron auth login
neuron
```

The installer downloads the native binary for Windows x64, Linux x64, or macOS Apple silicon from the matching GitHub release and checks its SHA-256 file. The first install requires access to GitHub Releases. Provider credentials stay on the Neuron gateway.

See the [NeuronCLI repository](https://github.com/RAHUL-DevelopeRR/neuron-v2) for supported systems and gateway setup.

Ponytail, antislop, gstack and Agent Reach instructions are embedded in the binary. Codebase Memory and Playwright MCP dependencies are installed with this package and enabled by default. Agent Reach's CLI channels require their own setup; Playwright provides browser control, while desktop control requires a separately configured MCP server. See [integration settings](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/ux-audit-fixes/docs/DEFAULT_INTEGRATIONS.md) for overrides and disabling defaults.
