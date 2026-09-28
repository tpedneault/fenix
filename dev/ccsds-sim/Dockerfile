# The simulator: one Python file, standard library only.
FROM python:3.12-slim
WORKDIR /sim
COPY sim.py .
RUN python sim.py --selftest
EXPOSE 10010 10011 10012 10013 10014
CMD ["python", "-u", "sim.py"]
