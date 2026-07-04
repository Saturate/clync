Output demo.gif
Set FontSize 14
Set Width 900
Set Height 450
Set Theme "Dracula"
Set TypingSpeed 40ms
Set Padding 16
Set Shell bash

Hide
Type "export GIT_AUTHOR_NAME=demo GIT_AUTHOR_EMAIL=demo@clync GIT_COMMITTER_NAME=demo GIT_COMMITTER_EMAIL=demo@clync"
Enter
Type "D=/tmp/clync-rec-$$ && rm -rf $D && mkdir -p $D"
Enter
Type "git init --bare -b main $D/r.git > /dev/null 2>&1"
Enter
Type `mkdir -p $D/a/home/.claude/projects/my-app $D/b/home/.claude/projects`
Enter
Type `printf '{"type":"mode","mode":"normal","sessionId":"abc-123"}\n{"uuid":"m1","type":"user","timestamp":1000,"message":{"content":"help me fix the login bug"}}\n{"uuid":"m2","parentUuid":"m1","type":"assistant","timestamp":2000,"message":{"content":"I found the issue in auth.ts"}}\n' > $D/a/home/.claude/projects/my-app/abc-123.jsonl`
Enter
Type "C=$(pwd)/target/release/clync"
Enter
Type "HOME=$D/a/home XDG_CONFIG_HOME=$D/a/cfg $C init --no-encrypt --repo $D/a/repo > /dev/null 2>&1 && cd $D/a/repo && git remote add origin $D/r.git && cd - > /dev/null"
Enter
Type `A() { HOME=$D/a/home XDG_CONFIG_HOME=$D/a/cfg $C "$@"; }`
Enter
Type `B() { HOME=$D/b/home XDG_CONFIG_HOME=$D/b/cfg $C "$@"; }`
Enter
Type `export PS1='\[\e[1;34m\]user@machine-1\[\e[0m\] $ '`
Enter
Sleep 300ms
Show

Type "clync push"
Enter
Hide
Type `A push 2>/dev/null | grep "^push:"`
Enter
Sleep 300ms
Show
Sleep 1.5s

Type "clync list"
Enter
Hide
Type "A list"
Enter
Sleep 300ms
Show
Sleep 2s

Hide
Type `export PS1='\[\e[1;35m\]user@machine-2\[\e[0m\] $ '`
Enter
Type `echo y | B join $D/r.git --no-encrypt --repo $D/b/repo > /dev/null 2>&1`
Enter
Sleep 300ms
Show

Type "clync list"
Enter
Hide
Type "B list"
Enter
Sleep 300ms
Show
Sleep 2s

Hide
Type `BS=$(find $D/b/home/.claude/projects -name "abc-123.jsonl" | head -1) && echo '{"uuid":"m3","parentUuid":"m2","type":"user","timestamp":3000,"message":{"content":"can you also add tests?"}}' >> $BS`
Enter
Show

Type "clync push"
Enter
Hide
Type `B push 2>/dev/null | grep "^push:"`
Enter
Sleep 300ms
Show
Sleep 1.5s

Hide
Type `echo '{"uuid":"m4","parentUuid":"m2","type":"user","timestamp":3500,"message":{"content":"ship it to staging"}}' >> $D/a/home/.claude/projects/my-app/abc-123.jsonl`
Enter
Type `export PS1='\[\e[1;34m\]user@machine-1\[\e[0m\] $ '`
Enter
Sleep 300ms
Show

Type "clync pull"
Enter
Hide
Type `A pull 2>/dev/null | grep "^pull:"`
Enter
Sleep 300ms
Show
Sleep 1.5s

Type "clync log"
Enter
Hide
Type "A log"
Enter
Sleep 300ms
Show
Sleep 2.5s

Hide
Type `echo ""; echo "✓ $(grep -c '"uuid"' $D/a/home/.claude/projects/my-app/abc-123.jsonl) messages merged across machines, no data lost"`
Enter
Sleep 200ms
Show
Sleep 3s
