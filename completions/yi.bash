# yi(1) bash completion. Static: lexopt has no generator (X10).
_yi() {
    local commands='ask rpc acp undo lanes trust gate fetch catalog doctor gc debug why plan todo memory sessions stats serve console tui version update plugin login logout setup mcp'
    local flags='--help --version --model --system --thinking --json --no-deltas --yolo --auto --confirm --allow-write --session-dir --cwd --here --fix --socket --headless --lanes --solo --keys --control --frames --faux --record --snap --size --eval --deadline --continue --session --schema --since --by --top'
    local word=${COMP_WORDS[COMP_CWORD]}
    if [[ $word == -* ]]; then
        COMPREPLY=($(compgen -W "$flags" -- "$word"))
        return
    fi
    case ${COMP_WORDS[1]} in
        sessions)
            if [[ $COMP_CWORD == 2 ]]; then
                COMPREPLY=($(compgen -W 'list show rm' -- "$word"))
                return
            fi
            ;;
    esac
    if [[ $COMP_CWORD == 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$word"))
    fi
}
complete -F _yi yi
