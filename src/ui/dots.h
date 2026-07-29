#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_DOTS (xd_dots_get_type ())
G_DECLARE_FINAL_TYPE (XdDots, xd_dots, XD, DOTS, AdwBin)

/*
 * Work happening, as three dots that fill in and start again.
 *
 * A spinner is a picture of a machine being busy; an ellipsis is what someone
 * writes when they have not finished the sentence yet -- which is what an
 * agent mid-turn actually is. It is also still, in the sense that matters: a
 * ring rotating at the edge of vision in a sidebar full of rows pulls the eye
 * to whichever one is turning, and there is nothing there to look at.
 *
 * The animation runs only while the widget is on screen, so rows scrolled out
 * of view and windows in the background cost nothing.
 */
XdDots *xd_dots_new (void);
void    xd_dots_set_animated (XdDots   *self,
                              gboolean  animated);

G_END_DECLS
