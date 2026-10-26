#!/usr/bin/env bash
# Hosted Ubuntu runners only; never invoked by the installed product.
set -euo pipefail
if [[ "$(dpkg --print-architecture)" == amd64 ]]; then
  # The Azure mirror has repeatedly stalled GTK/WebKit downloads. Keep the
  # Ubuntu archive and suite, using the main HTTPS mirror on x86_64 only.
  for source in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    [[ -f "$source" ]] || continue
    sudo sed -i -E 's#https?://azure\.archive\.ubuntu\.com/ubuntu#https://archive.ubuntu.com/ubuntu#g' "$source"
  done
fi
apt_options=(-o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 -o Acquire::Retries=2 -o APT::Update::Error-Mode=any)
sudo apt-get "${apt_options[@]}" update
sudo apt-get "${apt_options[@]}" install -y "$@"
