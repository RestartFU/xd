#include "text-reveal.h"

#define INITIAL_DELAY_USEC (80 * 1000)
#define TAIL_DELAY_USEC    (100 * 1000)
#define TRAILING_CHARS     2

void
xd_text_reveal_reset (XdTextReveal *self)
{
  g_return_if_fail (self != NULL);

  *self = (XdTextReveal) { 0 };
}

void
xd_text_reveal_note_append (XdTextReveal *self,
                            gint64        now)
{
  g_return_if_fail (self != NULL);

  if (self->reveal_after == 0)
    self->reveal_after = now + INITIAL_DELAY_USEC;
  self->last_append = now;
}

guint
xd_text_reveal_advance (XdTextReveal *self,
                        const char   *text,
                        gint64        now,
                        gboolean     *settled)
{
  guint total;
  guint target;
  guint pending;
  gboolean quiet;

  g_return_val_if_fail (self != NULL, 0);

  total = text != NULL ? (guint) g_utf8_strlen (text, -1) : 0;
  self->shown = MIN (self->shown, total);
  quiet = self->last_append == 0 ||
          now - self->last_append >= TAIL_DELAY_USEC;

  if (self->reveal_after != 0 && now < self->reveal_after)
    {
      if (settled != NULL)
        *settled = FALSE;
      return self->shown;
    }

  target = quiet ? total : total > TRAILING_CHARS
                           ? total - TRAILING_CHARS : 0;
  pending = target > self->shown ? target - self->shown : 0;

  if (pending > 0)
    {
      /* Drain a burst over several frames, but accelerate with its size so a
       * large network chunk never leaves the display seconds behind. */
      guint step = (pending + 2) / 3;

      if (quiet && pending <= 4)
        step = pending;
      self->shown += step;
    }

  if (settled != NULL)
    *settled = quiet && self->shown == total;

  return self->shown;
}

char *
xd_text_reveal_prefix (const char *text,
                       guint       characters)
{
  const char *end;
  guint total;

  if (text == NULL)
    return g_strdup ("");

  total = (guint) g_utf8_strlen (text, -1);
  end = g_utf8_offset_to_pointer (text, MIN (characters, total));

  return g_strndup (text, end - text);
}
