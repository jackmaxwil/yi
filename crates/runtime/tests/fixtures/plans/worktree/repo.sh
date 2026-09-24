#!/bin/sh
# The plan section 6.6 fixture repository: one parent checkout, three candidate
# branches, one moved parent generation and, on request, a dirty user tree.
# git and sh only, so a T1 lane test can build it without the kernel venv.
#
#   sh repo.sh <dir> [clean|dirty|staged|nested]
#
# builds <dir>/parent and leaves every branch in it. It refuses a <dir> that
# already exists: a half built fixture is worse than none.
set -eu

dir=${1:?usage: sh repo.sh <dir> [clean|dirty|staged|nested]}
dirt=${2:-clean}
if [ -e "$dir" ]; then
    echo "repo.sh: $dir already exists" >&2
    exit 2
fi
mkdir -p "$dir"
repo=$dir/parent
mkdir "$repo"

git -C "$repo" init -q -b main
git -C "$repo" config user.email worktree@fixture
git -C "$repo" config user.name worktree

# One tracked file. Line 2 is what a candidate writes and what the frozen
# checker greps; the last line is what the parent generation moves. The three
# filler lines between them are load bearing: git merges by hunk with three
# lines of context, so two edits any closer together conflict on their
# adjacency alone and the clean candidate would fail for the wrong reason.
write() {
    printf '#!/bin/sh\n# subcommands: list %s\n#\n# the launcher is a stub until the rest of the plan lands\n#\n# retention: %s days\n' "$1" "$2" >"$repo/rotate.sh"
}

write "" 7
git -C "$repo" add rotate.sh
git -C "$repo" commit -qm "the launcher lists"
base=$(git -C "$repo" rev-parse HEAD)

# Three candidates, each a branch off the same base with one commit on it. The
# green one also adds a file under a new directory: what a ref-only publication
# must still write into the parent checkout when no edit of the user's overlaps
# it, and must leave alone when the user's own untracked file is at that path.
candidate() {
    git -C "$repo" checkout -q -b "$1" "$base"
    write "$2" "$3"
    if [ "$1" = yi/cand-green ]; then
        mkdir "$repo/docs"
        printf '# rotate\n\nRotates the logs.\n' >"$repo/docs/ROTATE.md"
        git -C "$repo" add docs/ROTATE.md
    fi
    git -C "$repo" commit -qam "$4"
    git -C "$repo" checkout -q main
}

candidate yi/cand-green rotate 7 "add rotate"
candidate yi/cand-red compress 7 "add compress"
candidate yi/cand-conflict rotate 30 "add rotate and keep a month"

# The parent generation moves after the candidates are cut, on line 3 only.
write "" 14
git -C "$repo" commit -qam "keep two weeks"

# The user's uncommitted work: an edit to the file every candidate touches,
# unstaged or staged, and an untracked file; or an untracked directory holding
# the user's own file at the path the green candidate adds.
case $dirt in
clean) ;;
nested)
    mkdir "$repo/docs"
    printf 'MY PRECIOUS UNCOMMITTED WORK\n' >"$repo/docs/ROTATE.md"
    ;;
dirty | staged)
    printf '# staged by nobody\n' >>"$repo/rotate.sh"
    printf 'what I was in the middle of\n' >"$repo/scratch.txt"
    if [ "$dirt" = staged ]; then
        git -C "$repo" add rotate.sh
    fi
    ;;
*)
    echo "repo.sh: the dirt is clean, dirty, staged or nested, not $dirt" >&2
    exit 2
    ;;
esac
