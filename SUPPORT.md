# Getting help

Start with the [README](README.md) and [documentation index](docs/README.md).
Search existing [issues](https://github.com/ryltsevmikhail/dsh-ocean/issues) before
opening a bug report or feature request. Support is volunteer, best-effort, with
no response-time commitment.

Include Ocean version/commit, Linux distribution, architecture, desktop/session
(X11 or Wayland), relevant GTK/WebKit versions, install method and reproduction
steps. Say whether DSH works in a normal browser and whether Ocean uses an existing
service or its managed child. A minimal synthetic reproduction is preferable to a
large diagnostic archive.

`dsh-ocean --version`, `--runtime` and `--doctor` can help, but inspect their output
before posting. Remove private paths, hostnames, usernames, token URLs, cookies,
API credentials and journal/profile/session content. Do not attach your `.dsh`
directory or profile backups. Never grant someone account access to reproduce an
issue. Use [SECURITY.md](SECURITY.md) for suspected vulnerabilities.

Ocean-specific window, lifecycle, packaging and integration problems belong here.
Upstream DSH UI/model/plugin problems may need an upstream report: first reproduce
without Ocean, redact data and check the upstream project's reporting instructions.
Do not assume this repository can fix or support third-party services.
