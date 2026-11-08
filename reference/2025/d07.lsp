(defun read-lines (file)
  (with-open-file (s file)
    (loop for l = (read-line s nil) while l collect l)))

(let* ((lines (read-lines "y25d07.txt"))
       (counts (map 'vector (lambda (c) (if (char= c #\S) 1 0)) (first lines)))
       (n (length counts))
       (result1 0))
  (dolist (line (rest lines))
    (loop with snap = (copy-seq counts)
          for x below n for c across line
          when (and (char= c #\^) (plusp (aref snap x))) do
            (incf result1)
            (setf (aref counts x) 0)
            (when (plusp x) (incf (aref counts (1- x)) (aref snap x)))
            (when (< x (1- n)) (incf (aref counts (1+ x)) (aref snap x)))))
  (format t "Result1: ~A~%" result1)
  (format t "Result2: ~A~%" (reduce #'+ counts)))
