#include "dots.h"

/* Slow enough to read as writing rather than as flashing. */
#define STEP_MS 400

/* Every frame is the full width, so nothing beside it moves as it cycles. */
static const char *const frames[] = {
  "<span alpha='30%'>...</span>",
  ".<span alpha='30%'>..</span>",
  "..<span alpha='30%'>.</span>",
  "...",
};

struct _XdDots
{
  AdwBin parent_instance;

  GtkLabel *label;
  guint at;
  guint tick_id;
  gboolean animated;
};

G_DEFINE_FINAL_TYPE (XdDots, xd_dots, ADW_TYPE_BIN)

static gboolean
on_tick (gpointer user_data)
{
  XdDots *self = user_data;

  self->at = (self->at + 1) % G_N_ELEMENTS (frames);
  gtk_label_set_markup (self->label, frames[self->at]);

  return G_SOURCE_CONTINUE;
}

static void
start_tick (XdDots *self)
{
  if (self->animated &&
      gtk_widget_get_mapped (GTK_WIDGET (self)) &&
      self->tick_id == 0)
    self->tick_id = g_timeout_add (STEP_MS, on_tick, self);
}

/*
 * A mapped widget may still sit outside a scrolled viewport. Owners pause it
 * when that happens; map/unmap handles cached pages and hidden windows.
 */
static void
on_map (GtkWidget *widget)
{
  XdDots *self = XD_DOTS (widget);

  GTK_WIDGET_CLASS (xd_dots_parent_class)->map (widget);
  start_tick (self);
}

static void
on_unmap (GtkWidget *widget)
{
  XdDots *self = XD_DOTS (widget);

  g_clear_handle_id (&self->tick_id, g_source_remove);

  GTK_WIDGET_CLASS (xd_dots_parent_class)->unmap (widget);
}

static void
xd_dots_dispose (GObject *object)
{
  XdDots *self = XD_DOTS (object);

  g_clear_handle_id (&self->tick_id, g_source_remove);

  G_OBJECT_CLASS (xd_dots_parent_class)->dispose (object);
}

static void
xd_dots_class_init (XdDotsClass *klass)
{
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  G_OBJECT_CLASS (klass)->dispose = xd_dots_dispose;

  widget_class->map = on_map;
  widget_class->unmap = on_unmap;
}

static void
xd_dots_init (XdDots *self)
{
  self->animated = TRUE;
  self->label = GTK_LABEL (gtk_label_new (NULL));

  gtk_label_set_markup (self->label, frames[0]);
  gtk_widget_set_valign (GTK_WIDGET (self->label), GTK_ALIGN_CENTER);

  adw_bin_set_child (ADW_BIN (self), GTK_WIDGET (self->label));
}

XdDots *
xd_dots_new (void)
{
  return g_object_new (XD_TYPE_DOTS, NULL);
}

void
xd_dots_set_animated (XdDots   *self,
                      gboolean  animated)
{
  g_return_if_fail (XD_IS_DOTS (self));

  animated = !!animated;
  if (self->animated == animated)
    return;

  self->animated = animated;
  if (animated)
    start_tick (self);
  else
    g_clear_handle_id (&self->tick_id, g_source_remove);
}
