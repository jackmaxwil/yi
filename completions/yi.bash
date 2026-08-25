# yi(1) bash completion. Static: lexopt has no generator (X10).
_yi() {
    local commands='ask sessions undo rpc acp serve tui mcp version'
    local flags='--model --system --thinking --json --yolo --confirm --session --session-dir --continue --schema --cwd --socket --headless --keys --frames --version'
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
