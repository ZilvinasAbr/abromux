# abromux starts zsh with ZDOTDIR pointing here, so zsh reads this file
# first. Put the user's ZDOTDIR back (zsh then reads their .zprofile, .zshrc
# and .zlogin as usual), source their .zshenv, then add the prompt marks.
if [[ -n ${ABROMUX_ZSH_ZDOTDIR+X} ]]; then
    builtin export ZDOTDIR=$ABROMUX_ZSH_ZDOTDIR
    builtin unset ABROMUX_ZSH_ZDOTDIR
else
    builtin unset ZDOTDIR
fi

{
    builtin typeset _abromux_file=${ZDOTDIR-$HOME}/.zshenv
    [[ ! -r $_abromux_file ]] || builtin source -- $_abromux_file
} always {
    if [[ -o interactive ]]; then
        # ${(%):-%x} is this file; :A:h its directory.
        _abromux_file=${${(%):-%x}:A:h}/abromux.zsh
        [[ ! -r $_abromux_file ]] || builtin source -- $_abromux_file
    fi
    builtin unset _abromux_file
}
