#include "panel-style.h"

static const char PANEL_STYLE[] =
  ".xd-panel {"
  "  background: #0b0b0b;"
  "  border-radius: 14px;"
  "  border: 1px solid alpha(#ffffff, 0.07);"
  "  box-shadow: 0 24px 64px alpha(#000000, 0.65);"
  "}\n"

  ".xd-panel-bar { padding: 13px 16px; }\n"
  ".xd-panel-head { border-bottom: 1px solid alpha(#ffffff, 0.06); }\n"
  ".xd-panel-foot { border-top: 1px solid alpha(#ffffff, 0.06); }\n"

  /* Monochrome, because everything else on these panels is. */
  ".xd-panel-action {"
  "  background: alpha(#ffffff, 0.10);"
  "  border: 1px solid alpha(#ffffff, 0.08);"
  "  border-radius: 9px;"
  "  padding: 5px 14px;"
  "  box-shadow: none;"
  "}\n"
  ".xd-panel-action:hover { background: alpha(#ffffff, 0.16); }\n"

  ".xd-key { font-size: 85%; padding: 1px 6px; border-radius: 6px;"
  " background: alpha(#ffffff, 0.09); }\n";

void
xd_panel_style_ensure (void)
{
  static gsize once = 0;

  if (g_once_init_enter (&once))
    {
      g_autoptr (GtkCssProvider) provider = gtk_css_provider_new ();

      gtk_css_provider_load_from_string (provider, PANEL_STYLE);
      gtk_style_context_add_provider_for_display (
        gdk_display_get_default (), GTK_STYLE_PROVIDER (provider),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

      g_once_init_leave (&once, 1);
    }
}
