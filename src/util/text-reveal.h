#pragma once

#include <glib.h>

G_BEGIN_DECLS

#define XD_TEXT_REVEAL_FRAME_MSEC 33

typedef struct
{
  guint shown;
  gint64 reveal_after;
  gint64 last_append;
} XdTextReveal;

void  xd_text_reveal_reset       (XdTextReveal *self);
void  xd_text_reveal_note_append (XdTextReveal *self,
                                  gint64        now);
guint xd_text_reveal_advance     (XdTextReveal *self,
                                  const char   *text,
                                  gint64        now,
                                  gboolean     *settled);
char *xd_text_reveal_prefix      (const char   *text,
                                  guint         characters);

G_END_DECLS
