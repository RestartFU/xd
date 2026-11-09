# Recovery from a missing chat reply

Actual Linux application captures at 1280 × 800, dark theme, on an isolated
Xvfb display. All projects, messages and drafts are synthetic.

- `before.png`: base `8a3f0a2f`, 55 seconds after selecting the chat. The
  connection remains open but the fixture withholds chat metadata and history.
  The transcript and composer stay loading indefinitely.
- `after.png`: the same missing replies trigger the 30-second deadline. The
  client reconnects, reloads metadata and history, and restores the saved draft.

`host_fixture.py` implements the synthetic stdio host. Set
`XD_HOST_EXECUTABLE` to its absolute path and `XD_RECOVERY_PROOF` to a fresh
empty scratch directory. Run each app with separate `XDG_CONFIG_HOME`,
`XDG_DATA_HOME` and `XDG_CACHE_HOME`, select the fixture chat, and wait
40 seconds. Its `connections` counter should remain 1 before this fix and
reach 2 after it. `requests.jsonl` records only operations and timing;
pressing Enter writes the synthetic message into `submitted.txt`.

The mobile connection actor already applies a 30-second ordinary-call deadline,
abandons timed-out reply slots and reconnects a silent connection. This patch
adds desktop recovery for unanswered chat snapshots; it changes no layouts.
