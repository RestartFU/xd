# Answered question caption

A Claude chat whose reply ends with a tagged question, answered by tapping
"eu-west-1".

Desktop: actual GPUI application captures in Xvfb (lavapipe), 1400 × 900, Dark
theme, cropped to the transcript. A scripted Claude sends the reply through the
real host. Before is built from origin/master ecb6f060, after from this branch;
same fixture, viewport and clicks.

Android: Robolectric renders of the phone chat screen against a scripted host
(AnsweredQuestionScreenshotTest), 412 × 915 dp, Dark and Light, scaled to half
size. The same test ran on origin/master for the before captures.

- `*-before-*.png`: the answer is a bare bubble.
- `*-after-*.png`: the answer carries the question it answers above its bubble.
