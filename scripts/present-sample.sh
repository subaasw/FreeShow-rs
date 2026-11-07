#!/bin/sh
# Open a sample song and send its first slide to the output window.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
SAMPLE=$(mktemp -d)
mkdir -p "$SAMPLE/Shows" "$SAMPLE/Config"
cat > "$SAMPLE/Shows/Amazing Grace.show" <<'EOF'
["id-amazing",{"name":"Amazing Grace","settings":{"activeLayout":"main","template":null},"slides":{"verse":{"group":"Verse","color":null,"settings":{},"notes":"","items":[{"style":"","lines":[{"align":"","text":[{"value":"Amazing grace, how sweet the sound","style":""}]}]}]},"chorus":{"group":"Chorus","color":null,"settings":{},"notes":"","items":[{"style":"","lines":[{"align":"","text":[{"value":"Praise God","style":""}]}]}]}},"layouts":{"main":{"name":"Default","notes":"","slides":[{"id":"verse"},{"id":"chorus"}]}},"media":{}}]
EOF
printf '%s\n' '{"projects":{"p1":{"name":"Sunday","created":1,"parent":"/","shows":[{"id":"id-amazing","type":"show"}]}},"folders":{}}' > "$SAMPLE/Config/projects.json"
cd "$ROOT"
exec cargo run -p freeshow -- --data "$SAMPLE" --show "$SAMPLE/Shows/Amazing Grace.show"
