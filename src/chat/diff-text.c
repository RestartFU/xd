#include "diff-text.h"

struct _XdDiffText
{
  GtkWidget parent_instance;

  GtkWidget *label;
  GArray *rows;   /* guint8 XdDiffLineKind, one per layout line */
};

G_DEFINE_FINAL_TYPE (XdDiffText, xd_diff_text, GTK_TYPE_WIDGET)

static void
append_row_block (GtkSnapshot *snapshot,
                  const char  *colour,
                  float        top,
                  float        bottom,
                  float        width)
{
  GdkRGBA rgba;

  if (colour == NULL || bottom <= top || width <= 0)
    return;

  if (!gdk_rgba_parse (&rgba, colour))
    return;

  gtk_snapshot_append_color (
    snapshot, &rgba,
    &GRAPHENE_RECT_INIT (0, top, width, bottom - top));
}

/*
 * Consecutive rows of one colour become one rectangle.
 *
 * Drawing each row separately left hairlines between them wherever a row
 * boundary fell between two device pixels, which is the seam this widget
 * exists to remove.
 */
static void
xd_diff_text_snapshot (GtkWidget   *widget,
                       GtkSnapshot *snapshot)
{
  XdDiffText *self = XD_DIFF_TEXT (widget);
  int width = gtk_widget_get_width (widget);

  if (self->rows != NULL && self->rows->len > 0 && width > 0)
    {
      PangoLayout *layout = gtk_label_get_layout (GTK_LABEL (self->label));
      PangoLayoutIter *iter = pango_layout_get_iter (layout);
      gboolean chunk =
        gtk_widget_has_css_class (self->label, "xd-diff-chunk");
      const graphene_point_t label_point = GRAPHENE_POINT_INIT (0, 0);
      graphene_point_t label_origin;
      const char *block_colour = NULL;
      float block_top = 0;
      float block_bottom = 0;
      int origin_y = 0;
      guint row = 0;

      /*
       * GtkLabel's coordinates begin at its content box. CSS padding moves
       * that box inside this parent, so layout offsets alone are not parent
       * coordinates. Inline diffs have vertical padding; ignoring this
       * transform painted every changed-row background above its text.
       */
      if (!gtk_widget_compute_point (
            self->label, widget, &label_point, &label_origin))
        graphene_point_init (&label_origin, 0, 0);

      gtk_label_get_layout_offsets (
        GTK_LABEL (self->label), NULL, &origin_y);

      do
        {
          /* Both are literals from the same function, so the pointers say
           * whether two rows belong to the same block of colour. */
          const char *colour = row < self->rows->len
            ? xd_diff_line_background (
                g_array_index (self->rows, guint8, row))
            : NULL;
          int top_units = 0;
          int bottom_units = 0;
          float top;
          float bottom;

          pango_layout_iter_get_line_yrange (
            iter, &top_units, &bottom_units);
          top = label_origin.y + origin_y +
                (float) top_units / PANGO_SCALE;
          bottom = label_origin.y + origin_y +
                   (float) bottom_units / PANGO_SCALE;

          if (chunk && row == 0 && colour != NULL)
            top = 0;

          if (colour != block_colour)
            {
              append_row_block (
                snapshot, block_colour, block_top, block_bottom, width);
              block_colour = colour;
              block_top = top;
            }

          block_bottom = bottom;
          row++;
        }
      while (pango_layout_iter_next_line (iter));

      /*
       * Pango's last line can end fractionally before GTK's rounded widget
       * allocation. Adjacent virtual chunks then expose that remainder as a
       * random one-pixel black seam. Chunk labels have no vertical padding,
       * so extending a coloured edge to the allocation boundary is exact.
       */
      if (chunk && block_colour != NULL)
        block_bottom = MAX (
          block_bottom, (float) gtk_widget_get_height (widget));

      append_row_block (
        snapshot, block_colour, block_top, block_bottom, width);
      pango_layout_iter_free (iter);
    }

  GTK_WIDGET_CLASS (xd_diff_text_parent_class)->snapshot (widget, snapshot);
}

GtkWidget *
xd_diff_text_new (void)
{
  return g_object_new (XD_TYPE_DIFF_TEXT, NULL);
}

void
xd_diff_text_set_rows (XdDiffText *self,
                       const char *markup,
                       GArray     *row_kinds)
{
  g_return_if_fail (XD_IS_DIFF_TEXT (self));

  gtk_label_set_markup (
    GTK_LABEL (self->label), markup != NULL ? markup : "");

  g_clear_pointer (&self->rows, g_array_unref);
  self->rows = row_kinds != NULL ? g_array_ref (row_kinds) : NULL;

  gtk_widget_queue_draw (GTK_WIDGET (self));
}

GtkLabel *
xd_diff_text_get_label (XdDiffText *self)
{
  g_return_val_if_fail (XD_IS_DIFF_TEXT (self), NULL);

  return GTK_LABEL (self->label);
}

static void
xd_diff_text_dispose (GObject *object)
{
  XdDiffText *self = XD_DIFF_TEXT (object);

  g_clear_pointer (&self->label, gtk_widget_unparent);
  g_clear_pointer (&self->rows, g_array_unref);

  G_OBJECT_CLASS (xd_diff_text_parent_class)->dispose (object);
}

static void
xd_diff_text_class_init (XdDiffTextClass *klass)
{
  GtkWidgetClass *widget_class = GTK_WIDGET_CLASS (klass);

  G_OBJECT_CLASS (klass)->dispose = xd_diff_text_dispose;
  widget_class->snapshot = xd_diff_text_snapshot;
  gtk_widget_class_set_layout_manager_type (widget_class, GTK_TYPE_BIN_LAYOUT);
}

static void
xd_diff_text_init (XdDiffText *self)
{
  self->label = gtk_label_new (NULL);

  gtk_label_set_selectable (GTK_LABEL (self->label), TRUE);
  gtk_label_set_xalign (GTK_LABEL (self->label), 0.0f);
  gtk_label_set_yalign (GTK_LABEL (self->label), 0.0f);
  gtk_widget_set_hexpand (self->label, TRUE);
  gtk_widget_add_css_class (self->label, "xd-diff-text");
  gtk_widget_set_parent (self->label, GTK_WIDGET (self));
}
