# Security policy

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Report it
privately to the Trovul project maintainer with:

- a concise description and affected version;
- reproduction steps or a proof of concept;
- the security impact you believe is possible.

The maintainer will acknowledge the report, assess it, and coordinate a fix
before public disclosure where practical. Trovul does not currently offer a
bug bounty or a guaranteed response time.

## Security boundary

Trovul is designed for PCs you control on a trusted local network. It should
not be exposed directly to the internet. Features that accept peer connections,
share clipboard or files, control input, or expose the Companion API should be
enabled only for devices and networks you trust.
