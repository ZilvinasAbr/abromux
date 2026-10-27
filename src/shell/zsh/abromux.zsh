# Marks zsh's prompt with OSC 133 so the terminal knows which rows are the
# prompt. On resize it then blanks those rows for zsh to redraw, instead of
# reflowing them and leaving copies behind zsh's redraw.
#
#   133;A;redraw=1  prompt starts (and zsh redraws it on resize)
#   133;B           prompt ends, typed input starts
#   133;C           command output starts
#   133;D;<status>  command finished

[[ -o interactive ]] || builtin return 0
(( ! $+_abromux_state )) || builtin return 0

# 0: no command running, 1: output started (C sent, D not yet).
builtin typeset -gi _abromux_state=0
# PS1/PS2 before and after our marks, to spot a theme replacing them.
builtin typeset -g _abromux_ps1 _abromux_ps2 _abromux_marked_ps1 _abromux_marked_ps2

_abromux_precmd() {
    builtin local -i ret=$?
    builtin emulate -L zsh -o no_aliases

    # Run last so the marks go around whatever PS1 other hooks (themes) set.
    # Being moved takes effect from the next prompt.
    [[ ${precmd_functions[-1]} == _abromux_precmd ]] ||
        precmd_functions=(${precmd_functions:#_abromux_precmd} _abromux_precmd)

    # A precmd hook run from zle (some plugins do) isn't a finished command.
    if ! builtin zle && (( _abromux_state == 1 )); then
        builtin print -n "\e]133;D;$ret\a"
        _abromux_state=0
    fi

    [[ -o prompt_percent ]] || builtin return 0
    # Start from the unmarked prompt unless something replaced it.
    [[ $PS1 != $_abromux_marked_ps1 ]] || PS1=$_abromux_ps1
    [[ $PS2 != $_abromux_marked_ps2 ]] || PS2=$_abromux_ps2
    _abromux_ps1=$PS1 _abromux_ps2=$PS2
    # A trailing lone % would swallow the { of %{ below.
    [[ $PS1 == *[^%]% || $PS1 == % ]] && PS1=$PS1%
    [[ $PS2 == *[^%]% || $PS2 == % ]] && PS2=$PS2%
    # %{...%} prints the mark with zero width. Newlines in PS1 need nothing:
    # the terminal marks rows after a prompt's newline as continuations.
    PS1=$'%{\e]133;A;redraw=1\a%}'$PS1$'%{\e]133;B\a%}'
    PS2=$'%{\e]133;P;k=s\a%}'$PS2$'%{\e]133;B\a%}'
    _abromux_marked_ps1=$PS1 _abromux_marked_ps2=$PS2
}

_abromux_preexec() {
    builtin emulate -L zsh -o no_aliases
    # Show other hooks and the command the user's own PS1/PS2.
    [[ $PS1 != $_abromux_marked_ps1 ]] || PS1=$_abromux_ps1
    [[ $PS2 != $_abromux_marked_ps2 ]] || PS2=$_abromux_ps2
    builtin print -n "\e]133;C\a"
    _abromux_state=1
}

builtin typeset -ag precmd_functions preexec_functions
precmd_functions+=(_abromux_precmd)
preexec_functions+=(_abromux_preexec)
