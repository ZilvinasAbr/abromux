# abromux starts bash as `bash --posix -l` with ENV pointing here: in POSIX
# mode an interactive bash reads only $ENV. Leave POSIX mode, read the
# startup files bash would have read, then add the prompt marks and the
# codex wrapper. (Ghostty's bash integration does the same.)
if [ -n "${ABROMUX_BASH_INJECT+X}" ]; then
    builtin unset ENV ABROMUX_BASH_INJECT
    if [ -n "${ABROMUX_BASH_ENV+X}" ]; then
        builtin export ENV="$ABROMUX_BASH_ENV"
        builtin unset ABROMUX_BASH_ENV
    fi
    builtin set +o posix
    builtin shopt -u inherit_errexit 2>/dev/null
    # POSIX mode's history file is ~/.sh_history, so abromux exported
    # bash's usual one; keep it but don't pass it on.
    if [[ -n ${ABROMUX_BASH_UNEXPORT_HISTFILE-} ]]; then
        builtin export -n HISTFILE
        builtin unset ABROMUX_BASH_UNEXPORT_HISTFILE
    fi
    # See INVOCATION in bash(1).
    if builtin shopt -q login_shell; then
        [[ ! -r /etc/profile ]] || builtin source /etc/profile
        for _abromux_file in ~/.bash_profile ~/.bash_login ~/.profile; do
            [[ -r $_abromux_file ]] && { builtin source "$_abromux_file"; break; }
        done
    else
        # The system bashrc's place depends on the distribution.
        for _abromux_file in /etc/bash.bashrc /etc/bash/bashrc /etc/bashrc; do
            [[ -r $_abromux_file ]] && { builtin source "$_abromux_file"; break; }
        done
        [[ ! -r ~/.bashrc ]] || builtin source ~/.bashrc
    fi
    builtin unset _abromux_file
fi

# Marks bash's prompt with OSC 133 so the terminal knows which rows are the
# prompt. On resize it then blanks those rows for readline to redraw,
# instead of reflowing them and leaving copies behind the redraw.
#
#   133;A;redraw=1  prompt starts (and bash redraws it on resize)
#   133;B           prompt ends, typed input starts
#   133;C           command output starts (from PS0)
#   133;D;<status>  command finished

[[ $- == *i* ]] || builtin return 0
[[ -z ${_abromux_marked_ps1+X} ]] || builtin return 0

_abromux_ps1= _abromux_ps2= _abromux_marked_ps1= _abromux_marked_ps2=
_abromux_first=1

_abromux_prompt() {
    builtin local ret=$?
    # The previous command's end; nothing ran before the first prompt.
    if [[ -z $_abromux_first ]]; then
        builtin printf '\e]133;D;%s\a' "$ret"
    fi
    _abromux_first=
    # The command abromux resumes here, once, at the first prompt.
    [[ -z ${ABROMUX_RESUME_FILE+X} ]] || _abromux_resume
    # Start from the unmarked prompt unless something replaced it (a theme
    # that sets PS1 on every prompt).
    [[ $PS1 != "$_abromux_marked_ps1" ]] || PS1=$_abromux_ps1
    [[ $PS2 != "$_abromux_marked_ps2" ]] || PS2=$_abromux_ps2
    _abromux_ps1=$PS1 _abromux_ps2=$PS2
    # \[...\] prints the mark with zero width.
    PS1='\[\e]133;A;redraw=1\a\]'$PS1'\[\e]133;B\a\]'
    PS2='\[\e]133;P;k=s\a\]'$PS2'\[\e]133;B\a\]'
    _abromux_marked_ps1=$PS1 _abromux_marked_ps2=$PS2
    # Keep the status for whatever runs after this in PROMPT_COMMAND.
    builtin return "$ret"
}

# Session restore (program_resume.rs): run the command a program here
# reported with OSC 7577, or a coding agent's resume command, after the
# startup files. abromux writes it to a file only this user can read: a
# format version, the folder, then each argument, each ending in NUL. It's
# read and removed, and the arguments run as they are, never as shell code:
# the program is looked up on PATH, so no function or builtin of the name
# runs instead. The C and D marks around it tell abromux when it ends.
_abromux_resume() {
    builtin local f=$ABROMUX_RESUME_FILE v dir x p
    builtin local -a a
    a=()
    builtin unset ABROMUX_RESUME_FILE
    [[ -f $f && -O $f ]] || builtin return 0
    {
        IFS= builtin read -r -d '' v && IFS= builtin read -r -d '' dir &&
            while IFS= builtin read -r -d '' x; do a+=("$x"); done
    } < "$f"
    builtin command rm -f -- "$f"
    [[ $v == 1 && ${#a[@]} -gt 0 ]] || builtin return 0
    if [[ -n $dir ]]; then
        # Its folder is gone: nothing to resume.
        [[ -d $dir ]] && builtin cd -- "$dir" || builtin return 0
    fi
    if [[ ${a[0]} == */* ]]; then
        p=${a[0]}
    else
        p=$(builtin type -P -- "${a[0]}") && [[ -n $p ]] || builtin return 0
    fi
    builtin printf '\e]133;C\a'
    builtin command -- "$p" "${a[@]:1}"
    builtin printf '\e]133;D;%s\a' "$?"
}

# Last in PROMPT_COMMAND, so the marks go around the PS1 the rest set. An
# array since bash 5.1; a string before.
if [[ $(builtin declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND+=(_abromux_prompt)
else
    PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}_abromux_prompt"
fi
# PS0 is printed after a command is read, before it runs.
PS0=${PS0-}$'\e]133;C\a'

# Codex's TUI otherwise joins a shared background server, which runs every
# session's hooks with the environment of the terminal that started it, so
# they all report to that one tab. --no-daemon keeps them in this terminal.
# Older Codex has no daemon and no such flag. (`function` so that an alias
# named codex can't expand here.)
function codex {
    if [[ -z ${_abromux_codex_no_daemon-} ]]; then
        _abromux_codex_no_daemon=0
        [[ $(builtin command codex --help 2>/dev/null) != *--no-daemon* ]] ||
            _abromux_codex_no_daemon=1
    fi
    case ${1-} in
        # These work with the daemon itself.
        agents|app|app-server|remote-control) ;;
        *) [[ $_abromux_codex_no_daemon == 0 ]] || builtin set -- --no-daemon "$@" ;;
    esac
    builtin command codex "$@"
}
