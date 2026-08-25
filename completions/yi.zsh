#compdef yi
# yi(1) zsh completion. Static: lexopt has no generator (X10).
_yi() {
    local -a commands flags
    commands=(
        'ask:run one turn and print the answer'
        'sessions:list, show, or remove stored sessions'
        'undo:restore the files the last turn changed'
        'rpc:speak the Pi JSON-RPC protocol on stdio'
        'acp:speak ACP v2 on stdio'
        'serve:run the session daemon'
        'tui:open the terminal UI'
        'mcp:one-shot MCP client calls'
        'version:print the version'
    )
    flags=(
        '--model[provider/id]:model:'
        '--system[extra system prompt]:text:'
        '--thinking[thinking level]:level:(off minimal low medium high)'
        '--json[emit the event stream as JSON]'
        '--yolo[skip permission prompts (the default)]'
        '--confirm[ask before every write or command]'
        '--session[resume this session id]:id:'
        '--session-dir[session storage root]:dir:_files -/'
        '--continue[resume the newest session for this directory]'
        '--schema[validate the answer against a JSON Schema]:schema:_files'
        '--cwd[working directory]:dir:_files -/'
        '--socket[daemon socket path]:path:_files'
        '--headless[drive the TUI without a terminal]'
        '--keys[TUI key script]:file:_files'
        '--frames[TUI frame dump directory]:dir:_files -/'
    )
    _arguments -C $flags '1: :->command' '*:: :->rest'
    case $state in
        command) _describe 'command' commands ;;
        rest)
            case $words[1] in
                sessions) _values 'subcommand' list show rm ;;
            esac
            ;;
    esac
}
_yi "$@"
