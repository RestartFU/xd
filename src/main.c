#include "hy-app.h"

int
main (int argc, char *argv[])
{
  g_autoptr (HyApplication) app = hy_application_new ();

  return g_application_run (G_APPLICATION (app), argc, argv);
}
