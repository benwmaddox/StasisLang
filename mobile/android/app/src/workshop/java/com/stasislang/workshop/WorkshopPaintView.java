package com.stasislang.workshop;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffXfermode;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.accessibility.AccessibilityNodeInfo;

import java.util.ArrayList;

final class WorkshopPaintView extends View {
    static final int MIN_CANVAS_DIMENSION = 16;
    static final int MAX_CANVAS_DIMENSION = 1024;
    private static final int MAX_HISTORY = 8;
    private Bitmap bitmap;
    private Canvas bitmapCanvas;
    private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint checker = new Paint();
    private final Paint keyboardCursorOuterPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint keyboardCursorInnerPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final ArrayList<Bitmap> undo = new ArrayList<>();
    private final ArrayList<Bitmap> redo = new ArrayList<>();
    private float previousX;
    private float previousY;
    private boolean erasing;
    private WorkshopAccessibilityPolicy.PaintCursor keyboardCursor;

    WorkshopPaintView(Context context, int width, int height, Bitmap initial) {
        super(context);
        validateDimensions(width, height);
        bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        bitmapCanvas = new Canvas(bitmap);
        if (initial != null) bitmapCanvas.drawBitmap(initial, 0.0f, 0.0f, null);
        stroke.setColor(Color.BLACK);
        stroke.setStrokeWidth(8.0f);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        stroke.setStrokeJoin(Paint.Join.ROUND);
        keyboardCursorOuterPaint.setColor(Color.WHITE);
        keyboardCursorOuterPaint.setStyle(Paint.Style.STROKE);
        keyboardCursorOuterPaint.setStrokeWidth(5.0f * getResources().getDisplayMetrics().density);
        keyboardCursorInnerPaint.setColor(Color.BLACK);
        keyboardCursorInnerPaint.setStyle(Paint.Style.STROKE);
        keyboardCursorInnerPaint.setStrokeWidth(2.0f * getResources().getDisplayMetrics().density);
        keyboardCursor = WorkshopAccessibilityPolicy.initialPaintCursor(width, height);
        setBackgroundColor(Color.rgb(50, 55, 64));
        setContentDescription("Paint canvas. Touch to draw, or use arrow keys to move the paint cursor and Space or Enter to draw. Labeled tools follow the canvas.");
        setFocusable(true);
        setFocusableInTouchMode(true);
        setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_YES);
    }

    int canvasWidth() { return bitmap.getWidth(); }
    int canvasHeight() { return bitmap.getHeight(); }
    int brushColor() { return stroke.getColor(); }
    float brushSize() { return stroke.getStrokeWidth(); }

    void setBrushColor(int color) {
        erasing = false;
        stroke.setXfermode(null);
        stroke.setColor(color);
    }

    void setBrushSize(float pixels) {
        stroke.setStrokeWidth(Math.max(1.0f, Math.min(96.0f, pixels)));
    }

    void setEraser(boolean enabled) {
        erasing = enabled;
        stroke.setXfermode(enabled ? new PorterDuffXfermode(PorterDuff.Mode.CLEAR) : null);
    }

    boolean isErasing() { return erasing; }

    void clearCanvas() {
        saveUndo();
        bitmap.eraseColor(Color.TRANSPARENT);
        invalidate();
    }

    void undo() {
        if (undo.isEmpty()) return;
        pushBounded(redo, copyBitmap());
        replaceBitmap(undo.remove(undo.size() - 1));
    }

    void redo() {
        if (redo.isEmpty()) return;
        pushBounded(undo, copyBitmap());
        replaceBitmap(redo.remove(redo.size() - 1));
    }

    void resizeCanvas(int width, int height) {
        validateDimensions(width, height);
        if (width == bitmap.getWidth() && height == bitmap.getHeight()) return;
        saveUndo();
        Bitmap resized = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        new Canvas(resized).drawBitmap(bitmap, 0.0f, 0.0f, null);
        bitmap.recycle();
        bitmap = resized;
        bitmapCanvas = new Canvas(bitmap);
        clampKeyboardCursor();
        requestLayout();
        invalidate();
    }

    Bitmap snapshot() { return copyBitmap(); }

    void dispose() {
        if (!bitmap.isRecycled()) bitmap.recycle();
        recycleAll(undo);
        recycleAll(redo);
        undo.clear();
        redo.clear();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        float scale = displayScale();
        float left = (getWidth() - bitmap.getWidth() * scale) * 0.5f;
        float top = (getHeight() - bitmap.getHeight() * scale) * 0.5f;
        float tile = 16.0f * scale;
        for (int y = 0; y < bitmap.getHeight(); y += 16) {
            for (int x = 0; x < bitmap.getWidth(); x += 16) {
                checker.setColor(((x / 16 + y / 16) & 1) == 0
                        ? Color.rgb(230, 230, 230) : Color.rgb(190, 190, 190));
                float drawX = left + x * scale;
                float drawY = top + y * scale;
                canvas.drawRect(drawX, drawY, Math.min(left + bitmap.getWidth() * scale, drawX + tile),
                        Math.min(top + bitmap.getHeight() * scale, drawY + tile), checker);
            }
        }
        canvas.save();
        canvas.translate(left, top);
        canvas.scale(scale, scale);
        canvas.drawBitmap(bitmap, 0.0f, 0.0f, null);
        canvas.restore();
        if (hasFocus() || isAccessibilityFocused()) {
            float cursorX = left + (keyboardCursor.x + 0.5f) * scale;
            float cursorY = top + (keyboardCursor.y + 0.5f) * scale;
            float radius = Math.max(8.0f, stroke.getStrokeWidth() * scale);
            canvas.drawCircle(cursorX, cursorY, radius, keyboardCursorOuterPaint);
            canvas.drawCircle(cursorX, cursorY, radius, keyboardCursorInnerPaint);
        }
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        int step = event.isShiftPressed() ? 8 : 1;
        if (keyCode == KeyEvent.KEYCODE_DPAD_LEFT) return moveKeyboardCursor(-step, 0);
        if (keyCode == KeyEvent.KEYCODE_DPAD_RIGHT) return moveKeyboardCursor(step, 0);
        if (keyCode == KeyEvent.KEYCODE_DPAD_UP) return moveKeyboardCursor(0, -step);
        if (keyCode == KeyEvent.KEYCODE_DPAD_DOWN) return moveKeyboardCursor(0, step);
        if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER || keyCode == KeyEvent.KEYCODE_SPACE
                || keyCode == KeyEvent.KEYCODE_ENTER) {
            paintAtKeyboardCursor();
            return true;
        }
        return super.onKeyDown(keyCode, event);
    }

    @Override
    public void onInitializeAccessibilityNodeInfo(AccessibilityNodeInfo info) {
        super.onInitializeAccessibilityNodeInfo(info);
        info.addAction(new AccessibilityNodeInfo.AccessibilityAction(
                R.id.paint_cursor_left, "Move paint cursor left"));
        info.addAction(new AccessibilityNodeInfo.AccessibilityAction(
                R.id.paint_cursor_right, "Move paint cursor right"));
        info.addAction(new AccessibilityNodeInfo.AccessibilityAction(
                R.id.paint_cursor_up, "Move paint cursor up"));
        info.addAction(new AccessibilityNodeInfo.AccessibilityAction(
                R.id.paint_cursor_down, "Move paint cursor down"));
        info.addAction(new AccessibilityNodeInfo.AccessibilityAction(
                R.id.paint_at_cursor, erasing ? "Erase at paint cursor" : "Paint at cursor"));
    }

    @Override
    public boolean performAccessibilityAction(int action, Bundle arguments) {
        if (action == R.id.paint_cursor_left) return moveKeyboardCursor(-1, 0);
        if (action == R.id.paint_cursor_right) return moveKeyboardCursor(1, 0);
        if (action == R.id.paint_cursor_up) return moveKeyboardCursor(0, -1);
        if (action == R.id.paint_cursor_down) return moveKeyboardCursor(0, 1);
        if (action == R.id.paint_at_cursor) {
            paintAtKeyboardCursor();
            return true;
        }
        return super.performAccessibilityAction(action, arguments);
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        float scale = displayScale();
        float left = (getWidth() - bitmap.getWidth() * scale) * 0.5f;
        float top = (getHeight() - bitmap.getHeight() * scale) * 0.5f;
        float x = (event.getX() - left) / scale;
        float y = (event.getY() - top) / scale;
        boolean inside = x >= 0.0f && y >= 0.0f && x < bitmap.getWidth() && y < bitmap.getHeight();
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN && inside) {
            getParent().requestDisallowInterceptTouchEvent(true);
            saveUndo();
            previousX = x;
            previousY = y;
            keyboardCursor = WorkshopAccessibilityPolicy.movePaintCursor(
                    new WorkshopAccessibilityPolicy.PaintCursor(Math.round(x), Math.round(y)),
                    0, 0, bitmap.getWidth(), bitmap.getHeight());
            bitmapCanvas.drawPoint(x, y, stroke);
            invalidate();
            return true;
        }
        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            if (!inside) return true;
            bitmapCanvas.drawLine(previousX, previousY, x, y, stroke);
            previousX = x;
            previousY = y;
            invalidate();
            return true;
        }
        if (event.getActionMasked() == MotionEvent.ACTION_UP
                || event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            getParent().requestDisallowInterceptTouchEvent(false);
            if (event.getActionMasked() == MotionEvent.ACTION_UP) performClick();
            return true;
        }
        return super.onTouchEvent(event);
    }

    @Override
    public boolean performClick() {
        super.performClick();
        return true;
    }

    private boolean moveKeyboardCursor(int deltaX, int deltaY) {
        WorkshopAccessibilityPolicy.PaintCursor next =
                WorkshopAccessibilityPolicy.movePaintCursor(keyboardCursor,
                deltaX, deltaY, bitmap.getWidth(), bitmap.getHeight());
        if (next.x == keyboardCursor.x && next.y == keyboardCursor.y) return false;
        keyboardCursor = next;
        invalidate();
        announceForAccessibility("Paint cursor " + keyboardCursor.x + ", " + keyboardCursor.y);
        return true;
    }

    private void paintAtKeyboardCursor() {
        saveUndo();
        bitmapCanvas.drawPoint(keyboardCursor.x, keyboardCursor.y, stroke);
        invalidate();
        announceForAccessibility((erasing ? "Erased" : "Painted") + " at "
                + keyboardCursor.x + ", " + keyboardCursor.y);
    }

    private float displayScale() {
        if (getWidth() <= 0 || getHeight() <= 0) return 1.0f;
        return Math.min((float)getWidth() / bitmap.getWidth(), (float)getHeight() / bitmap.getHeight());
    }

    private void saveUndo() {
        pushBounded(undo, copyBitmap());
        recycleAll(redo);
        redo.clear();
    }

    private Bitmap copyBitmap() {
        return bitmap.copy(Bitmap.Config.ARGB_8888, true);
    }

    private void replaceBitmap(Bitmap replacement) {
        bitmap.recycle();
        bitmap = replacement;
        bitmapCanvas = new Canvas(bitmap);
        clampKeyboardCursor();
        requestLayout();
        invalidate();
    }

    private static void pushBounded(ArrayList<Bitmap> history, Bitmap snapshot) {
        history.add(snapshot);
        if (history.size() > MAX_HISTORY) history.remove(0).recycle();
    }

    private void clampKeyboardCursor() {
        keyboardCursor = WorkshopAccessibilityPolicy.movePaintCursor(keyboardCursor,
                0, 0, bitmap.getWidth(), bitmap.getHeight());
    }

    private static void recycleAll(ArrayList<Bitmap> history) {
        for (Bitmap bitmap : history) bitmap.recycle();
    }

    private static void validateDimensions(int width, int height) {
        if (width < MIN_CANVAS_DIMENSION || height < MIN_CANVAS_DIMENSION
                || width > MAX_CANVAS_DIMENSION || height > MAX_CANVAS_DIMENSION) {
            throw new IllegalArgumentException("paint canvas must be 16-1024 pixels on each axis");
        }
    }
}
