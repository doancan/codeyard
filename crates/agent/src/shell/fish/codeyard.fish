# Codeyard shell integration (fish): loaded from vendor_conf.d.
if status is-interactive; and test "$TERM_PROGRAM" = codeyard; and not set -q __cy_loaded
    set -g __cy_loaded 1
    function __cy_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
    function __cy_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
    function __cy_prompt --on-event fish_prompt
        printf '\e]7;file://%s%s\a' (hostname) (string replace -a '%' '%25' -- "$PWD")
        printf '\e]133;A\a'
    end
end
