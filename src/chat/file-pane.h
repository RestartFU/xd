#pragma once

#include <adwaita.h>

#include "remote/client.h"

G_BEGIN_DECLS

#define XD_TYPE_FILE_PANE (xd_file_pane_get_type ())

G_DECLARE_FINAL_TYPE (XdFilePane, xd_file_pane, XD, FILE_PANE, AdwBin)

XdFilePane *xd_file_pane_new         (void);
void        xd_file_pane_set_workdir (XdFilePane     *self,
                                      const char     *workdir);
void        xd_file_pane_set_remote  (XdFilePane     *self,
                                      XdRemoteClient *client,
                                      const char     *chat_id);
void        xd_file_pane_refresh     (XdFilePane     *self);

G_END_DECLS
