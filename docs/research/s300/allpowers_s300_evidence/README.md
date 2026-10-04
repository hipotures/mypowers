# S300 protocol evidence

Captured on 2026-10-04 for `AP S300 V2.0`, station `2A:02:01:48:6B:D0`,
through the Actions adapter `F4:4E:FC:A1:CB:FF`. Hardware/firmware version
numbers remain unknown. See `../ALLPOWERS_S300_PROTOCOL.md` for interpretation.

- `regression_vectors.json`: three canonical output round trips and two earlier
  historical-profile round trips. Each operation links its raw capture sequence,
  starting status, exact TX, two subsequent status notifications and physical
  observation. Restoration is recorded at the round-trip level.
- `experiments.json`: request, cross-field side effect, selector restoration,
  generic app fields and invalid-checksum trials. This is not a write allowlist.
- `captures/`: raw Bleak application events, plus standalone probe stdout.
  These are not HCI or official-app captures. The AC probe stdout is an explicitly
  annotated excerpt; the DC/light probe stdout is complete. Probe streams do not
  include UTC timestamps; supervised lab captures do.
- `manifest.json`: hashes of archived captures, external camera paths and static
  app artifacts. APKs, app source and camera images are not redistributed.

The canonical command allowlist is `a56500b10101001869`,
`a56500b10101001a6b`, `a56500b10101001b6a`, `a56500b10101003a4b`.
Initial canonical status is `0e`; states outside the captured profile are rejected
by the standalone control probe. Historical three-field frames are recorded as
evidence, not approved general controls.

Historical harness labels preserve what was being investigated at the time.
`set usb on` was later rejected as a USB interpretation. `confirmed` for an
unchanged-state experimental trial means that two baseline statuses arrived;
it does not prove acceptance or support of that candidate.

Battery, power and remaining minutes are dynamic. Compare envelope, length,
checksum and expected flags instead of requiring a future full status frame to
equal one captured here. Camera pictures may be blurred or saturated; their
individual limitations are annotated in the vectors/document.

All photographs remain in `/tmp/allpowers-camera/` at the user's request.
`/tmp` paths can disappear; hashes preserve identity, not image contents.
