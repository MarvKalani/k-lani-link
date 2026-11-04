# VM acceptance — 2 November 2026

Test machine: `k-lani-link-mxc`, Windows 11 Home x64 **26200.9711**.
Account: the standard local user `tester`, **not elevated**.
Programs: Microsoft MXC **1.0.1**, official OpenCode **1.19.5**,
k-lani-link **1.2611.2-preview.2**. Binary/archive hashes are pinned in
`release.json`. The existing VM control panel and separate scratch host supplied
the real model connection; the existing model container was not reconfigured.

## Passed

`Start-OpenCode.ps1 -CheckIsolation` returned exit code 0, **12/12**:

- Read and write a temporary project marker, then remove it.
- Read the pinned tool executable; fail to write into its directory.
- Fail to read or append to a known existing file outside the granted paths.
- Reach the exact local API TCP endpoint and retrieve its authenticated model list.
- Fail to connect to other live host listeners over IPv4 and IPv6.
- Fail direct external IPv4/IPv6 TCP attempts and HTTPS through the configured proxy.

A real OpenCode `run` invocation then used the Qwen Coder model through the tunnel,
called its `write` tool twice, and created a self-contained `index.html` with a
click counter. It ended with `MXC_AGENT_OK` and exit code 0. Independent inspection
confirmed a complete HTML document without external script/style links.
The first generated file's SHA-256 was
`fb03b266fe48c4145da098b10f2ac3d31d2fc37868aa9633d7520eabf66f04df`.
The operator did not write or repair the generated page.

A subsequent interactive `--mini` session read that file and added a Reset button
with OpenCode's edit tools. It finished with `MXC_VIDEO_OK`. An independent
JavaScript check ran the generated click handlers: two clicks produced 2 and
Reset returned the counter to 0, without editing the source. The edited file's
SHA-256 is `eaaf8d28a711ffec7aee794e3b116293b204cc8b76f75a84d4eb33eabf0086d8`.
The VM browser also confirmed the counter displaying 2 and Reset returning it to 0.
The interactive run took about six minutes; the short video labels accelerated
waiting time and cuts.

## Rejected alternatives

- Without proxy mode, a direct deny-default rule allowing only TCP 8787 still
  allowed access to a different live host loopback port. An explicit deny rule
  did not close it either. These policies are not used by this launcher.
- Proxy mode closed the other port but blocked host-to-container UI connections
  and connections to another TCP listener inside the sandbox itself.
- OpenCode 2.2.0's standalone terminal mode therefore failed. Its internal
  `serve --stdio --port 0` process requires this additional TCP connection.
  OpenCode 1.19.5's `--mini` / `run` use an in-process server instead.
- Bun needs metadata access to ancestor directories to resolve Windows paths.
  These receive only `enumeratePaths`; content grants remain on the selected
  project, private session home and read-only tools. The outside-file rejection
  was retested after adding these metadata permissions.

## Limits of this evidence

This verifies this Windows build and these pinned programs, not every customer
machine. External IPv6 failure alone does not demonstrate an independently
working host IPv6 route; the live IPv6 loopback test has a listening control.
The local API key intentionally enters the sandbox's process environment.
Windows system resources and ancestor path metadata remain available as described
in the main guide. Shell tools are disabled; no compiler/package-manager workflow
was certified. MXC calls the unpackaged local proxy arrangement a development/test
compatibility deployment. See [README.md](README.md) for setup and boundaries.

The video and generated fixtures are private artifacts, outside the source tree.
