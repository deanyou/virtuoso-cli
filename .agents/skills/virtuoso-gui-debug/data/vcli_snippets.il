; vcli_snippets.il — Auto-promoted from RSI knowledge base
; Load in Virtuoso: load("<path>/vcli_snippets.il")

; draw_rect → vcliDrawRect()
procedure(vcliDrawRect(layer purpose x1 y1 x2 y2)
  ; Draw a rectangle on current cellview
  let(((cv geGetEditCellView())) dbCreateRect(cv list(layer purpose) list(x1:y1 x2:y2)))
endprocedure

; draw_polygon → vcliDrawPolygon()
procedure(vcliDrawPolygon(layer purpose points)
  ; Draw a polygon from point list
  let(((cv geGetEditCellView())) dbCreatePolygon(cv list(layer purpose) list(points)))
endprocedure

; draw_path → vcliDrawPath()
procedure(vcliDrawPath(layer purpose points width)
  ; Draw a path with given width
  let(((cv geGetEditCellView())) dbCreatePath(cv list(layer purpose) list(points) width))
endprocedure

; draw_line → vcliDrawLine()
procedure(vcliDrawLine(layer purpose x1 y1 x2 y2)
  ; Draw a line segment
  let(((cv geGetEditCellView())) dbCreateLine(cv list(layer purpose) list(x1:y1 x2:y2)))
endprocedure

; save_cv → vcliSaveCv()
procedure(vcliSaveCv()
  ; Save current cellview
  dbSave(geGetEditCellView())
endprocedure

; zoom_fit → vcliZoomFit()
procedure(vcliZoomFit()
  ; Zoom to fit in current window
  hiWorldViewFit()
endprocedure

; select_all → vcliSelectAll()
procedure(vcliSelectAll()
  ; Select all objects in current window
  geSelectAll()
endprocedure

; deselect_all → vcliDeselectAll()
procedure(vcliDeselectAll()
  ; Deselect all objects
  geDeselectAll()
endprocedure

; delete_selected → vcliDeleteSelected()
procedure(vcliDeleteSelected()
  ; Delete currently selected objects
  foreach(s geGetSelSet() dbDeleteObject(s))
endprocedure
