# Neuron Mobile (skeleton)

A remote control for a Neuron session that runs on your computer. You can:

- pair with a host
- watch the transcript live: messages, tool calls with their state and diff size, questions
- approve or deny tool calls
- send prompts and slash commands
- interrupt

It shares the session reducer and the remote protocol with the desktop app (`ui/neuron-client`),
so both show the same session at once. The first answer to a permission request wins.

Expo SDK 58 / React Native 0.88. There is no router yet: two screens, Connect and Session.

## Try it

1. On the computer, start a host. In `ui/neuron-client`, run `bun run host --mock --lan` for the
   mock engine, or `bun run host --lan --cwd <project>` for your own. It prints a LAN address and
   the path of a token file. Add `--show-token` to print the token itself.
2. Here:

   ```bash
   bun run start
   ```

   Open the app in Expo Go, then enter the address and the token. `bun run web` opens it in a
   browser instead.

LAN traffic is not encrypted, so use the LAN host only on networks you trust. The zero-x.live
relay, with end-to-end encryption and pairing by QR code, is planned: see
`docs/NEURON_APPS_PLAN.md` §5. Native builds (EAS, the Android or iOS SDKs) come later.

## Tests

```bash
bun test
```

This covers pairing (address forms, wrong tokens) and a phone steering a host: a prompt, then a
permission answer reaching the engine.
