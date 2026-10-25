#compdef yi
# yi(1) zsh completion. Static: lexopt has no generator (X10).
_yi() {
    local -a commands flags
    commands=(
        'ask:run one turn and print the answer'
        'rpc:speak the Pi JSON-RPC protocol on stdio'
        'acp:speak ACP v2 on stdio'
        'undo:restore the files the last turn changed'
        'lanes:list the lane slots, or reap one'
        'trust:trust this repository, or list or revoke'
        'gate:say whether a command would run, and why'
        'fetch:resolve one URL through the wall'
        'catalog:show or refresh the model catalog'
        'doctor:check session invariants, --fix repairs'
        'gc:preview the retention sweep, --fix runs it'
        'debug:info, paths, config, profile, trace, select'
        'why:trace a line or a todo back to its goal'
        'plan:plan ops as the owner'
        'todo:print the newest todo list'
        'memory:list, search, import or forget memories'
        'sessions:list, show, or remove stored sessions'
        'stats:tools and tokens of a session, or a rollup'
        'serve:run the session daemon'
        'console:open the workspace shell'
        'tui:open the solo terminal UI'
        'version:print the version'
        'update:replace this install from the latest release'
        'plugin:list, add, remove or sync pinned programs'
        'login:store a provider credential'
        'logout:remove a provider credential'
        'setup:choose a model and a permission mode'
        'mcp:one-shot MCP client calls, under mcp.enabled'
    )
    flags=(
        '--help[print this table (also -h)]'
        '--version[print the version]'
        '--model[the model, over the configured one]:provider/id:'
        '--system[extra system prompt]:text:'
        '--thinking[off, minimal, low, medium, high, xhigh, max]:level:(off minimal low medium high xhigh max)'
        '--json[print JSON lines (the event stream, for ask)]'
        '--yolo[run without permission prompts]'
        '--auto[let the policy judge each call (the default)]'
        '--confirm[ask before every write or command]'
        '--session-dir[session storage root]:dir:_files -/'
        '--cwd[working directory]:dir:_files -/'
        '--here[work in this directory and claim no lane]'
        '--fix[let doctor or gc change what it reports]'
        '--socket[daemon socket path]:path:_files'
        '--headless[drive the tui or console without a terminal]'
        '--lanes[claim a lane in a headless or cassette run]'
        '--solo[bare yi opens the tui, not the console]'
        '--keys[headless key script]:file:_files'
        '--control[drive an interactive run from a socket]:socket:_files'
        '--frames[headless frame dump directory]:dir:_files -/'
        '--faux[replay a reply cassette, no model call]:file:_files'
        '--record[record the headless drive as an asciicast]:file:_files'
        '--snap[one still frame of the headless drive]:file:_files'
        '--size[headless screen size, 80x24 by default]:COLSxROWS:'
        '--eval[an eval harness run, which reads YI_LEVERS]'
        '--deadline[wall-clock limit for the run]:secs:'
        '--continue[resume the newest session for this directory]'
        '--session[resume this session]:id:'
        '--schema[answer JSON valid against this schema]:schema:_files'
        '--since[after stats: fold the sessions of this window]:span:'
        '--by[after stats: group by model, upstream, day, prices or plan]:axis:'
        '--top[after stats: list the n costliest sessions]:n:'
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
