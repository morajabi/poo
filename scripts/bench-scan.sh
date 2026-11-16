#!/usr/bin/env zsh
# Repeatable scan benchmark. Read-only against real folders: every run uses a throwaway
# POO_HOME, so poo's own state (index, report, answered folders) never touches yours.
#
#   scripts/bench-scan.sh real    # your home, with a copy of your config (scans only read)
#   scripts/bench-scan.sh synth   # a generated tree of APFS clones in $TMPDIR (< 100 MB real)
#
# POO=path/to/poo picks the binary (default: target/release/poo). RUNS=n repeats each step.
set -euo pipefail
setopt null_glob
zmodload zsh/datetime
mode=${1:-synth}
here=${0:A:h}/..
POO=${POO:-$here/target/release/poo}
RUNS=${RUNS:-3}
export POO_NO_UPDATE_CHECK=1 NO_COLOR=1
unset XDG_CONFIG_HOME XDG_STATE_HOME POO_CONFIG_DIR POO_STATE_DIR

work=${BENCH_DIR:-${TMPDIR:-/tmp}/poo-bench}
mkdir -p $work
export POO_HOME=$work/poo-home-$mode
rm -rf $POO_HOME
mkdir -p $POO_HOME/config

# Times one command: prints "label  seconds".
t() {
  local label=$1; shift
  local s=$EPOCHREALTIME
  "$@" >/dev/null 2>$work/last.err || { echo "$label failed:"; cat $work/last.err; exit 1 }
  printf "  %-34s %7.3f s\n" $label $(( EPOCHREALTIME - s ))
}

# A tree of projects: node_modules, target, .git, sources, a few big build files.
make_synth() {
  local root=$work/synth
  if [[ -f $root/.done ]]; then echo $root; return; fi
  rm -rf $root; mkdir -p $root/tpl
  python3 -I - $root/tpl <<'EOF'
import os, sys
t = sys.argv[1]
def w(p, n):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f: f.write(b"x" * n)
w(f"{t}/package.json", 200)
w(f"{t}/Cargo.toml", 200)
for i in range(60):
    for j in range(25):
        w(f"{t}/node_modules/pkg{i}/lib/sub{j % 5}/f{j}.js", 900)
    w(f"{t}/node_modules/pkg{i}/package.json", 300)
for i in range(20):
    for j in range(20):
        w(f"{t}/target/debug/deps/c{i}/o{j}.rlib", 4000)
w(f"{t}/target/debug/big.bin", 8_000_000)
for i in range(40):
    w(f"{t}/src/mod{i % 8}/file{i}.rs", 1500)
for i in range(10):
    w(f"{t}/docs/d{i}.md", 500)
EOF
  git -C $root/tpl init -q && git -C $root/tpl add src package.json Cargo.toml docs \
    && git -C $root/tpl -c user.name=b -c user.email=b@b commit -qm init
  printf "node_modules\ntarget\n" > $root/tpl/.gitignore
  for g in 1 2 3 4 5; do
    mkdir -p $root/dev/group$g
    for p in $(seq 1 24); do cp -cR $root/tpl $root/dev/group$g/proj$p; done
  done
  rm -rf $root/tpl
  touch $root/.done
  echo $root
}

if [[ $mode == real ]]; then
  [[ -f ~/.config/poo/config.toml ]] && cp ~/.config/poo/config.toml $POO_HOME/config/
  echo "real home, POO_HOME=$POO_HOME, binary $POO"
else
  root=$(make_synth)
  # Undo the previous run's edits so every run sees the same tree.
  rm -f $root/dev/group3/proj7/target/debug/deps/c3/new*.rlib
  rm -rf $root/dev/group2/proj*/node_modules/added*
  printf 'roots = ["%s/dev"]\n' $root > $POO_HOME/config/config.toml
  echo "synthetic tree $root ($(find $root/dev | wc -l | tr -d ' ') entries), binary $POO"
fi

t "warm-up full scan" $POO scan --full --json
for i in $(seq $RUNS); do t "full scan" $POO scan --full --json; done
for i in $(seq $RUNS); do t "incremental, no change" $POO scan --json; done
if [[ $mode == synth ]]; then
  for i in $(seq $RUNS); do
    f=$root/dev/group3/proj7/target/debug/deps/c3/new$i.rlib
    head -c 2000000 /dev/zero > $f
    mkdir -p $root/dev/group2/proj$i/node_modules/added$i && echo x > $root/dev/group2/proj$i/node_modules/added$i/a.js
    t "incremental, after change" $POO scan --json
  done
fi
for i in $(seq $RUNS); do t "startup + overview (cached)" $POO overview --json; done
for i in $(seq $RUNS); do t "ls -n 0 (cached)" $POO ls -n 0 --json; done
ls -l $POO_HOME/state/index.json $POO_HOME/state/report.json | awk '{printf "  %-34s %7.1f MB\n", $NF, $5/1e6}'
