# Stage 1: Build the fat JAR using Maven
FROM maven:3.9-eclipse-temurin-21 AS build

WORKDIR /build

# Copy the POM first to cache dependency downloads
COPY pom.xml .
RUN mvn dependency:go-offline -q

# Copy source and build the shaded JAR
COPY src/ src/
RUN mvn package -q

# Stage 2: Slim runtime image with just JRE and the JAR
FROM eclipse-temurin:21-jre-alpine

WORKDIR /app

# Copy the shaded (fat) JAR from the build stage
COPY --from=build /build/target/liquibase-bridge-1.0.0.jar /app/liquibase-bridge.jar

ENTRYPOINT ["java", "-jar", "/app/liquibase-bridge.jar"]
