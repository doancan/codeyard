ZDOTDIR="$__cy_user"
[ -f "$ZDOTDIR/.zshrc" ] && . "$ZDOTDIR/.zshrc"
__cy_user="$ZDOTDIR"
ZDOTDIR="$__cy_dir"
if [[ -o interactive && -z "$__cy_loaded" && "$TERM_PROGRAM" == codeyard ]]; then
  __cy_loaded=1
  __cy_ran=
  __cy_precmd() {
    local ret=$?
    [[ -n "$__cy_ran" ]] && printf '\e]133;D;%s\a' "$ret"
    __cy_ran=
    printf '\e]7;file://%s%s\a' "${HOST}" "${PWD}"
    printf '\e]133;A\a'
    return $ret
  }
  __cy_preexec() {
    __cy_ran=1
    printf '\e]133;C\a'
  }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __cy_precmd
  add-zsh-hook preexec __cy_preexec
fi
