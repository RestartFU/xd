#pragma once

#include <gtk/gtk.h>

G_BEGIN_DECLS

/* GitHub-style, read-only unified diff rows. */
GtkWidget *xd_diff_view_new  (const char *patch,
                              gboolean    show_file_headers,
                              guint      *additions,
                              guint      *deletions);
void       xd_diff_view_fill (GtkBox     *box,
                              const char *patch,
                              gboolean    show_file_headers,
                              guint      *additions,
                              guint      *deletions);

/*
 * Virtualized, collapsible file sections for the persistent Git pane.
 * Sections start expanded and remember collapsed paths across refreshes.
 */
GtkWidget *xd_diff_view_new_file_sections  (void);
void       xd_diff_view_fill_file_sections (GtkListView *view,
                                            const char  *patch,
                                            guint       *additions,
                                            guint       *deletions);

G_END_DECLS
