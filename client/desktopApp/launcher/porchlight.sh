#!/bin/sh
# XWayland bridges need non-reparenting mode except GNOME/KDE/Cinnamon/Pantheon/Unity's reparenting WMs.
if [ "${_JAVA_AWT_WM_NONREPARENTING+x}" != x ]; then
    case ${PORCHLIGHT_AWT_NONREPARENTING-} in
        1) _JAVA_AWT_WM_NONREPARENTING=1; export _JAVA_AWT_WM_NONREPARENTING ;;
        0) ;;
        *)
            if [ -n "${WAYLAND_DISPLAY-}" ] || [ "${XDG_SESSION_TYPE-}" = wayland ]; then
                reparenting=0
                desktops=${XDG_CURRENT_DESKTOP-}:
                while [ -n "$desktops" ]; do
                    desktop=${desktops%%:*}
                    desktops=${desktops#*:}
                    case $desktop in
                        [gG][nN][oO][mM][eE]|[kK][dD][eE]|[cC][iI][nN][nN][aA][mM][oO][nN]|[xX]-[cC][iI][nN][nN][aA][mM][oO][nN]|[pP][aA][nN][tT][hH][eE][oO][nN]|[uU][nN][iI][tT][yY])
                            reparenting=1; break ;;
                    esac
                done
                if [ "$reparenting" = 0 ]; then
                    _JAVA_AWT_WM_NONREPARENTING=1
                    export _JAVA_AWT_WM_NONREPARENTING
                fi
            fi
            ;;
    esac
fi
case $0 in
    */*) launcher_directory=${0%/*} ;;
    *) launcher_directory=. ;;
esac
exec "$launcher_directory/porchlight-app" "$@"
