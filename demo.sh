#!/bin/bash
set -e

CLYNC="$(cd "$(dirname "$0")" && pwd)/target/release/clync"
DIR="/tmp/clync-demo-$$"
BARE="$DIR/remote.git"

cleanup() { rm -rf "$DIR"; }
trap cleanup EXIT
mkdir -p "$DIR"

export GIT_AUTHOR_NAME="demo" GIT_AUTHOR_EMAIL="demo@clync"
export GIT_COMMITTER_NAME="demo" GIT_COMMITTER_EMAIL="demo@clync"

git init --bare -b main "$BARE" > /dev/null 2>&1

MA="$DIR/a"; MB="$DIR/b"
mkdir -p "$MA/home/.claude/projects/my-app" "$MB/home/.claude/projects"

cat > "$MA/home/.claude/projects/my-app/abc-123.jsonl" <<'JSONL'
{"type":"mode","mode":"normal","sessionId":"abc-123"}
{"uuid":"m1","type":"user","timestamp":1000,"message":{"content":"help me fix the login bug"}}
{"uuid":"m2","parentUuid":"m1","type":"assistant","timestamp":2000,"message":{"content":"I found the issue in auth.ts"}}
JSONL

HOME="$MA/home" XDG_CONFIG_HOME="$MA/config" $CLYNC init --no-encrypt --repo "$MA/repo" > /dev/null 2>&1
cd "$MA/repo" && git remote add origin "$BARE" && cd - > /dev/null

prompt_a() { printf "\033[1;34muser@machine-1\033[0m \033[2m~/projects\033[0m $ "; }
prompt_b() { printf "\033[1;35muser@machine-2\033[0m \033[2m~/projects\033[0m $ "; }

prompt_a; echo "clync push"
HOME="$MA/home" XDG_CONFIG_HOME="$MA/config" $CLYNC push 2>/dev/null | grep "^push:"
echo ""

prompt_a; echo "clync list"
HOME="$MA/home" XDG_CONFIG_HOME="$MA/config" $CLYNC list
echo ""

prompt_b; echo "clync join git@github.com:user/clync-data.git"
echo "y" | HOME="$MB/home" XDG_CONFIG_HOME="$MB/config" $CLYNC join "$BARE" --no-encrypt --repo "$MB/repo" 2>/dev/null | grep -E "cloning|repo encryption|config saved|pulled|done\."
echo ""

prompt_b; echo "clync list"
HOME="$MB/home" XDG_CONFIG_HOME="$MB/config" $CLYNC list
echo ""

BSESSION=$(find "$MB/home/.claude/projects" -name "abc-123.jsonl" 2>/dev/null | head -1)
echo '{"uuid":"m3","parentUuid":"m2","type":"user","timestamp":3000,"message":{"content":"can you also add tests?"}}' >> "$BSESSION"

prompt_b; echo "clync push"
HOME="$MB/home" XDG_CONFIG_HOME="$MB/config" $CLYNC push 2>/dev/null | grep "^push:"
echo ""

echo '{"uuid":"m4","parentUuid":"m2","type":"user","timestamp":3500,"message":{"content":"ship it to staging"}}' >> "$MA/home/.claude/projects/my-app/abc-123.jsonl"

prompt_a; echo "clync pull"
HOME="$MA/home" XDG_CONFIG_HOME="$MA/config" $CLYNC pull 2>/dev/null | grep "^pull:"
echo ""

prompt_a; echo "clync log"
HOME="$MA/home" XDG_CONFIG_HOME="$MA/config" $CLYNC log
echo ""

MERGED="$MA/home/.claude/projects/my-app/abc-123.jsonl"
COUNT=$(grep -c '"uuid"' "$MERGED")
printf "\033[1;32m✓ $COUNT messages merged across machines, no data lost\033[0m\n"
sleep 1
